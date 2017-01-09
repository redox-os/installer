use std::collections::BTreeMap;

mod general;
mod package;
mod user;

#[derive(Debug, Default, Deserialize)]
pub struct Config {
    pub general: general::GeneralConfig,
    pub packages: BTreeMap<String, package::PackageConfig>,
    pub users: BTreeMap<String, user::UserConfig>,
}
