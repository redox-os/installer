#![deny(warnings)]

#[macro_use]
extern crate serde_derive;

pub use config::Config;
pub use install::install;

mod config;
mod install;
