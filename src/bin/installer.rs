extern crate arg_parser;
extern crate redox_installer;
extern crate serde;
extern crate toml;

use std::{env, fs, io, process};
use std::io::{Read, Write};
use std::path::Path;

use arg_parser::ArgParser;

use redox_installer::PackageConfig;

fn main() {
    let stderr = io::stderr();
    let mut stderr = stderr.lock();

    let mut parser = ArgParser::new(4)
        .add_opt("b", "cookbook")
        .add_opt("c", "config")
        .add_flag(&["p", "cooking"])
        .add_flag(&["l", "list-packages"])
        .add_flag(&["live"]);
    parser.parse(env::args());

    // Allow filesystem config to specify recipe or package, enabling mixed recipe/binary build
    let cooking = parser.found("cooking");

    let mut config_data = String::new();
    let mut config = if let Some(path) = parser.get_opt("config") {
        match fs::File::open(&path) {
            Ok(mut config_file) => {
                match config_file.read_to_string(&mut config_data) {
                    Ok(_) => {
                        match toml::from_str(&config_data) {
                            Ok(config) => {
                                config
                            },
                            Err(err) => {
                                writeln!(stderr, "installer: {}: failed to decode: {}", path, err).unwrap();
                                process::exit(1);
                            }
                        }
                    },
                    Err(err) => {
                        writeln!(stderr, "installer: {}: failed to read: {}", path, err).unwrap();
                        process::exit(1);
                    }
                }
            },
            Err(err) => {
                writeln!(stderr, "installer: {}: failed to open: {}", path, err).unwrap();
                process::exit(1);
            }
        }
    } else {
        redox_installer::Config::default()
    };

    // Add filesystem.toml to config
    config.files.push(redox_installer::FileConfig {
        path: "filesystem.toml".to_string(),
        data: config_data,
        ..Default::default()
    });

    // Add command line flags to config, command line takes priority
    if cooking {
        config.general.cooking = Some(true);
    }

    if parser.found("list-packages") {
        for (packagename, package) in &config.packages {
            if config.general.cooking == Some(true) {
                // Only print the names of packages that are relevant to the cookbook
                match package {
                    PackageConfig::Build(rule) if rule == "recipe" => {
                        println!("{}", packagename);
                    }
                    PackageConfig::Build(rule) if rule == "recipe_no_fetch" => {
                        println!("{}", packagename);
                    }
                    _ => {}
                }
            } else {
                println!("{}", packagename);
            }
        }
    } else {
        let cookbook = if let Some(path) = parser.get_opt("cookbook") {
            if ! Path::new(&path).is_dir() {
                writeln!(stderr, "installer: {}: cookbook not found", path).unwrap();
                process::exit(1);
            }

            // Add cookbook key to config
            let key_path = Path::new(&path).join("build/id_ed25519.pub.toml");
            match fs::read_to_string(&key_path) {
                Ok(data) => {
                    config.files.push(redox_installer::FileConfig {
                        path: "pkg/id_ed25519.pub.toml".to_string(),
                        data: data,
                        ..Default::default()
                    });
                    Some(path)
                },
                Err(err) => {
                    // if there are no recipes coming from the cookbook, this is not a fatal error
                    if config.packages.clone().into_iter().any(| (_packagename, package) |
                        match package {
                            PackageConfig::Empty => false,
                            PackageConfig::Spec { version: None, git: None, path: None, } => false,
                            _ => true,
                        })
                    {
                        writeln!(stderr, "installer: {}: failed to read cookbook key: {}", key_path.display(), err).unwrap();
                        process::exit(1);
                    } else {
                        writeln!(stderr, "installer: {}: (non-fatal) missing cookbook key: {}", key_path.display(), err).unwrap();
                        None
                    }
                }
            }

        } else {
            None
        };

        if let Some(path) = parser.args.get(0) {
            if let Err(err) = redox_installer::install(config, path, cookbook, parser.found("live")) {
                writeln!(stderr, "installer: failed to install: {}", err).unwrap();
                process::exit(1);
            }
        } else {
            writeln!(stderr, "installer: output or list-packages not found").unwrap();
            process::exit(1);
        }
    }
}
