extern crate arg_parser;
extern crate redox_installer;
extern crate serde;
extern crate toml;

use std::path::Path;
use std::{env, fs, process};

use arg_parser::ArgParser;

use redox_installer::{Config, PackageConfig};

fn main() {
    let mut parser = ArgParser::new(4)
        .add_opt("b", "cookbook")
        .add_opt("c", "config")
        .add_opt("o", "output-config")
        .add_opt("", "write-bootloader")
        .add_flag(&["filesystem-size"])
        .add_flag(&["r", "repo-binary"])
        .add_flag(&["l", "list-packages"])
        .add_flag(&["live"])
        .add_flag(&["no-mount"]);
    parser.parse(env::args());

    // Use pre-built binaries for packages as the default.
    // If not set on the command line or the filesystem config, then build packages from source.
    let repo_binary = parser.found("repo-binary");

    let mut config = if let Some(path) = parser.get_opt("config") {
        match Config::from_file(Path::new(&path)) {
            Ok(config) => config,
            Err(err) => {
                eprintln!("installer: {err}");
                process::exit(1);
            }
        }
    } else {
        redox_installer::Config::default()
    };

    // Get toml of merged config
    let merged_toml = toml::to_string_pretty(&config).unwrap();

    // Just output merged config and exit
    if let Some(path) = parser.get_opt("output-config") {
        fs::write(path, merged_toml).unwrap();
        return;
    }

    // Add filesystem.toml to config
    config.files.push(redox_installer::FileConfig {
        path: "filesystem.toml".to_string(),
        data: merged_toml,
        ..Default::default()
    });

    // Add command line flags to config, command line takes priority
    if repo_binary {
        config.general.repo_binary = Some(true);
    }

    if parser.found("filesystem-size") {
        println!("{}", config.general.filesystem_size.unwrap_or(0));
    } else if parser.found("list-packages") {
        // List the packages that should be fetched or built by the cookbook
        for (packagename, package) in &config.packages {
            match package {
                PackageConfig::Build(rule) if rule == "ignore" => {
                    // skip this package
                }
                _ => {
                    println!("{}", packagename);
                }
            }
        }
    } else {
        let cookbook = if let Some(path) = parser.get_opt("cookbook") {
            if !Path::new(&path).is_dir() {
                eprintln!("installer: {}: cookbook not found", path);
                process::exit(1);
            }

            // Add cookbook key to config
            let key_path = Path::new(&path).join("build/id_ed25519.pub.toml");
            match fs::read_to_string(&key_path) {
                Ok(data) => {
                    config.files.push(redox_installer::FileConfig {
                        path: "pkg/id_ed25519.pub.toml".to_string(),
                        data,
                        ..Default::default()
                    });
                    Some(path)
                }
                Err(err) => {
                    // if there are no recipes coming from the cookbook, this is not a fatal error
                    if config
                        .packages
                        .clone()
                        .into_iter()
                        .any(|(_packagename, package)| match package {
                            PackageConfig::Empty => false,
                            PackageConfig::Spec {
                                version: None,
                                git: None,
                                path: None,
                            } => false,
                            _ => true,
                        })
                    {
                        eprintln!(
                            "installer: {}: failed to read cookbook key: {}",
                            key_path.display(),
                            err
                        );
                        process::exit(1);
                    } else {
                        eprintln!(
                            "installer: {}: (non-fatal) missing cookbook key: {}",
                            key_path.display(),
                            err
                        );
                        None
                    }
                }
            }
        } else {
            None
        };

        if cookbook.is_some() {
            config.general.cookbook = cookbook;
        }
        if parser.found("live") {
            config.general.live_disk = Some(true);
        }
        if parser.found("no-mount") {
            config.general.no_mount = Some(true);
        }
        let write_bootloader = parser.get_opt("write-bootloader");
        if write_bootloader.is_some() {
            config.general.write_bootloader = write_bootloader;
        }

        if let Some(path) = parser.args.first() {
            if let Err(err) = redox_installer::install(config, path) {
                eprintln!("installer: failed to install: {}", err);
                process::exit(1);
            }
        } else {
            eprintln!("installer: output or list-packages not found");
            process::exit(1);
        }
    }
}
