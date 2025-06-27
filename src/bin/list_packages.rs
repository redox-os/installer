/// List packages for compilation, skip binary packages to be downloaded
extern crate arg_parser;
extern crate redox_installer;
extern crate serde;
extern crate toml;

use std::io::Write;
use std::path::Path;
use std::{env, io, process};

use arg_parser::ArgParser;

use redox_installer::{Config, PackageConfig};

fn main() {
    let stderr = io::stderr();
    let mut stderr = stderr.lock();

    let mut parser = ArgParser::new(4)
        .add_opt("c", "config")
        .add_flag(&["r", "repo-binary"]);
    parser.parse(env::args());

    // Use pre-built binaries for packages as the default.
    // If not set on the command line or the filesystem config, then build packages from source.
    let repo_binary = parser.found("repo-binary");

    let mut config = if let Some(path) = parser.get_opt("config") {
        match Config::from_file(Path::new(&path)) {
            Ok(config) => config,
            Err(err) => {
                writeln!(stderr, "installer: {err}").unwrap();
                process::exit(1);
            }
        }
    } else {
        redox_installer::Config::default()
    };

    // Get toml of merged config
    let merged_toml = toml::to_string_pretty(&config).unwrap();

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

    // List the packages that should be fetched or built by the cookbook
    for (packagename, package) in &config.packages {
        match package {
            PackageConfig::Build(rule) if rule == "recipe" || rule == "source" => {
                println!("{}", packagename);
            }
            PackageConfig::Build(rule) if rule == "binary" || rule == "ignore" => {
                // skip this package
            }
            _ => {
                if config.general.repo_binary == Some(true) {
                    // default action is to not build this package, skip it
                } else {
                    // default action is to build
                    println!("{}", packagename);
                }
            }
        }
    }
}
