#![deny(warnings)]

extern crate redox_installer;
extern crate serde;
extern crate toml;

use std::{env, process};
use std::fs::File;
use std::io::{self, Read, Write};

fn main() {
    let stderr = io::stderr();
    let mut stderr = stderr.lock();

    let mut configs = vec![];
    for arg in env::args().skip(1) {
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
        if let Err(err) = redox_installer::install(config) {
            writeln!(stderr, "installer: failed to install: {}", err).unwrap();
            process::exit(1);
        }
    }
}
