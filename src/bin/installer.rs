#![deny(warnings)]

extern crate clap;
extern crate redox_installer;
extern crate serde;
extern crate toml;

use std::process;
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::Path;

use clap::{App, Arg};

fn main() {
    let stderr = io::stderr();
    let mut stderr = stderr.lock();

    let matches = App::new("redox_installer")
        .arg(
            Arg::with_name("cookbook")
                .help("Path of cookbook")
                .short("b")
                .long("cookbook")
                .takes_value(true)
                .value_name("FOLDER")
        )
        .arg(
            Arg::with_name("config")
                .help("Configuration file")
                .short("c")
                .long("config")
                .takes_value(true)
                .value_name("FILE")
        )
        .arg(
            Arg::with_name("list-packages")
                .help("List packages")
                .short("l")
                .long("list-packages")
        )
        .arg(
            Arg::with_name("output")
                .help("Output folder or device")
                .index(1)
                .value_name("OUTPUT")
        )
        .get_matches();

    let config = if let Some(path) = matches.value_of("config") {
        match File::open(path) {
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
                                        config
                                    },
                                    Err(err) => {
                                        writeln!(stderr, "installer: {}: failed to decode: {}", path, err).unwrap();
                                        process::exit(1);
                                    }
                                }
                            },
                            None => {
                                for error in parser.errors {
                                    writeln!(stderr, "installer: {}: failed to parse: {}", path, error).unwrap();
                                }
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

    let cookbook = if let Some(path) = matches.value_of("cookbook") {
        if ! Path::new(&path).is_dir() {
            writeln!(stderr, "installer: {}: cookbook not found", path).unwrap();
            process::exit(1);

        }

        Some(path)
    } else {
        None
    };

    if matches.is_present("list-packages") {
        for (packagename, _package) in &config.packages {
            println!("{}", packagename);
        }
    } else {
        if let Some(path) = matches.value_of("output") {
            if let Err(err) = redox_installer::install(config, path, cookbook) {
                writeln!(stderr, "installer: failed to install: {}", err).unwrap();
                process::exit(1);
            }
        } else {
            writeln!(stderr, "installer: output or list-packages not found").unwrap();
            process::exit(1);
        }
    }
}
