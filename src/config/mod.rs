use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

pub mod file;
pub mod general;
pub mod package;
pub mod user;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Config {
    pub general: general::GeneralConfig,
    #[serde(default)]
    pub packages: BTreeMap<String, package::PackageConfig>,
    #[serde(default)]
    pub files: Vec<file::FileConfig>,
    #[serde(default)]
    pub users: BTreeMap<String, user::UserConfig>,
}

impl Config {
    pub fn from_file(path: &Path) -> Result<Self, failure::Error> {
        let config = match fs::read_to_string(&path) {
            Ok(config_data) => match toml::from_str(&config_data) {
                Ok(config) => config,
                Err(err) => {
                    return Err(format_err!("{}: failed to decode: {}", path.display(), err));
                }
            },
            Err(err) => {
                return Err(format_err!("{}: failed to read: {}", path.display(), err));
            }
        };
        Ok(config)
    }
}
