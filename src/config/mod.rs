use std::collections::BTreeMap;

pub mod general;
pub mod file;
pub mod package;
pub mod user;

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Config {
    pub general: general::GeneralConfig,
    #[serde(default)]
    pub packages: BTreeMap<String, package::PackageConfig>,
    #[serde(default)]
    pub files: Vec<file::FileConfig>,
    #[serde(default)]
    pub users: BTreeMap<String, user::UserConfig>,
}
