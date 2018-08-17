use std::collections::BTreeMap;

mod general;
pub(crate) mod file;
mod package;
mod user;

#[derive(Debug, Default, Deserialize)]
pub struct Config {
    pub general: general::GeneralConfig,
    pub packages: BTreeMap<String, package::PackageConfig>,
    pub files: Vec<file::FileConfig>,
    pub users: BTreeMap<String, user::UserConfig>,
}
