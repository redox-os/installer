#[macro_use]
extern crate serde_derive;

mod config;
#[cfg(feature = "installer")]
mod disk_wrapper;
#[cfg(feature = "installer")]
mod installer;
#[cfg(feature = "installer")]
pub use crate::installer::*;

pub use crate::config::file::FileConfig;
pub use crate::config::package::PackageConfig;
pub use crate::config::Config;
