#![deny(warnings)]

extern crate redox_installer;
extern crate serde;
extern crate toml;

use std::{env, process};
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::Path;

fn main() {
    let stderr = io::stderr();
    let mut stderr = stderr.lock();

    let mut configs = vec![];
    let mut cookbook = None;
    let mut list_packages = false;
    for arg in env::args().skip(1) {
        if arg.starts_with("--cookbook=") {
            let path = arg.splitn(2, "--cookbook=").nth(1).unwrap().to_string();
            if !Path::new(&path).is_dir() {
                writeln!(stderr, "installer: {}: cookbook not found", arg).unwrap();
                process::exit(1);

            }
            cookbook = Some(path);
            continue;
        }

        if arg == "--list-packages" {
            list_packages = true;
            continue;
        }

        match File::open(&arg) {
            Ok(mut config_file) => {
                let mut config_data = String::new();
                match config_file.read_to_string(&mut config_data) {
                    Ok(_) => {
                        let mut parser = toml::Parser::new(&config_data);
                        match parser.parse() {
                            Some(parsed) => {
                                let mut decoder = toml::Decoder::new(toml::Value::Table(parsed));
                                match serde::Deserialize::deserialize(&mut decoder) {
                                    Ok(config) => {
                                        configs.push(config);
                                    },
                                    Err(err) => {
                                        writeln!(stderr, "installer: {}: failed to decode: {}", arg, err).unwrap();
                                        process::exit(1);
                                    }
                                }
                            },
                            None => {
                                for error in parser.errors {
                                    writeln!(stderr, "installer: {}: failed to parse: {}", arg, error).unwrap();
                                }
                                process::exit(1);
                            }
                        }
                    },
                    Err(err) => {
                        writeln!(stderr, "installer: {}: failed to read: {}", arg, err).unwrap();
                        process::exit(1);
                    }
                }
            },
            Err(err) => {
                writeln!(stderr, "installer: {}: failed to open: {}", arg, err).unwrap();
                process::exit(1);
            }
        }
    }

    if configs.is_empty() {
        configs.push(redox_installer::Config::default());
    }

    for config in configs {
        if list_packages {
            for (packagename, _package) in &config.packages {
                println!("{}", packagename);
            }
        } else if let Err(err) = redox_installer::install(config, cookbook.as_ref().map(String::as_ref)) {
            writeln!(stderr, "installer: failed to install: {}", err).unwrap();
            process::exit(1);
        }
    }
}
