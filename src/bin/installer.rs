extern crate arg_parser;
extern crate redox_installer;
extern crate serde;
extern crate toml;

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::{env, fs, process};

use arg_parser::ArgParser;
use pkg::net_backend::DownloadBackend;

use redox_installer::{Config, PackageConfig};

const HELP_STR: &str = r#"
redox_installer - Redox Installer.
                  Refer to link below for filesystem config reference:
                  https://doc.redox-os.org/book/configuration-settings.html

Using redox_installer as an installer:
  redox_installer <diskpath.img> [--config=file.toml] [--write-bootloader=file.img] [--live] [--no-mount] [--skip-partition]
    <diskpath.img>        Disk file to write
    --config              Path to filesystem config TOML
    --write-bootloader    Path to write UEFI bootloader to in addition to the embedded ESP
    --skip-partition      Skip writing GPT partition tables
                          Use this only if you plan to use other partition tool
    --live                Use bootloader configured for live disk
    --no-mount            Use RedoxFS AR instead of FUSE to write files
    --cookbook            Use local Redox OS build system rather than downloading packages
    --config-name         Name of the filesystem configuration used for os-release VARIANT

Using redox_installer as a configuration parser:
  redox_installer --config=file.toml [--list-packages|--filesystem-size|--output-config path]
    --list-packages      List packages will be installed
    --filesystem-size    Output filesystem size in MB
    --output-config      Path to write the parsed config as another TOML
"#;

fn os_release_quote(value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{}\"", escaped)
}

fn build_id_from_repo_toml(repo_toml: &str) -> Option<String> {
    toml::from_str::<toml::Value>(repo_toml)
        .ok()?
        .get("build_id")?
        .as_str()
        .and_then(|value| {
            if value.is_empty() {
                None
            } else {
                Some(value.to_string())
            }
        })
}

fn local_repo_build_id(cookbook: &str) -> Option<String> {
    let repo_toml = Path::new(cookbook)
        .join("repo")
        .join(redox_installer::get_target())
        .join("repo.toml");
    let repo_toml = fs::read_to_string(repo_toml).ok()?;
    build_id_from_repo_toml(&repo_toml)
}

fn remote_repo_build_id() -> Option<String> {
    let callback = Rc::new(RefCell::new(pkg::callback::SilentCallback::new()));
    let download_backend = pkg::net_backend::DefaultNetBackend::new().ok()?;
    let mut repo = pkg::RepoManager::new(callback, Box::new(download_backend));
    repo.add_remote(
        "https://static.redox-os.org/pkg",
        &redox_installer::get_target(),
    )
    .ok()?;

    let package = pkg::PackageName::new("repo").ok()?;
    let (repo_toml, _) = repo.get_package_toml(&package).ok()?;
    build_id_from_repo_toml(&repo_toml)
}

fn append_os_release_metadata(
    config: &mut Config,
    config_name: Option<&str>,
    build_id: Option<String>,
) {
    let mut data = String::new();
    if let Some(config_name) = config_name.filter(|value| !value.is_empty()) {
        data.push_str("VARIANT=");
        data.push_str(&os_release_quote(config_name));
        data.push('\n');
    }
    if let Some(build_id) = build_id.filter(|value| !value.is_empty()) {
        data.push_str("BUILD_ID=");
        data.push_str(&os_release_quote(&build_id));
        data.push('\n');
    }

    if !data.is_empty() {
        config.files.push(redox_installer::FileConfig {
            path: "/usr/lib/os-release".to_string(),
            data,
            append: true,
            ..Default::default()
        });
    }
}

fn main() {
    let mut parser = ArgParser::new(4)
        .add_opt("b", "cookbook")
        .add_opt("", "config-name")
        .add_opt("c", "config")
        .add_opt("o", "output-config")
        .add_opt("", "write-bootloader")
        .add_flag(&["skip-partition"])
        .add_flag(&["filesystem-size"])
        .add_flag(&["r", "repo-binary"]) // TODO: Remove
        .add_flag(&["l", "list-packages"])
        .add_flag(&["live"])
        .add_flag(&["no-mount"]);
    parser.parse(env::args());

    let skip_partition = parser.found("skip-partition");

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

    if skip_partition {
        config.general.skip_partitions = Some(true);
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

            Some(path)
        } else {
            None
        };

        let build_id = if let Some(cookbook) = cookbook.as_deref() {
            local_repo_build_id(cookbook)
        } else {
            remote_repo_build_id()
        };
        append_os_release_metadata(
            &mut config,
            parser.get_opt("config-name").as_deref(),
            build_id,
        );

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
                eprintln!("installer: failed to install: {:?}", err);
                process::exit(1);
            }
        } else {
            eprint!("{}", HELP_STR);
            process::exit(1);
        }
    }
}
