use std::collections::BTreeMap;
use std::fs;
use std::mem;
use std::path::{Path, PathBuf};

pub mod file;
pub mod general;
pub mod package;
pub mod user;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Config {
    #[serde(default)]
    pub include: Vec<PathBuf>,
    #[serde(default)]
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
        let mut config: Config = match fs::read_to_string(&path) {
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

        let config_dir = path.parent().unwrap();

        let mut configs = mem::take(&mut config.include)
            .into_iter()
            .map(|path| Config::from_file(&config_dir.join(path)))
            .collect::<Result<Vec<Config>, failure::Error>>()?;
        configs.push(config); // Put ourself last to ensure that it overwrites anything else.

        config = configs.remove(0);

        for other_config in configs {
            config.merge(other_config);
        }

        Ok(config)
    }

    pub fn merge(&mut self, other: Config) {
        assert!(self.include.is_empty());
        assert!(other.include.is_empty());

        let Config {
            include: _,
            general: other_general,
            packages: other_packages,
            files: other_files,
            users: other_users,
        } = other;

        self.general.merge(other_general);

        for (package, package_config) in other_packages {
            self.packages.insert(package, package_config);
        }

        self.files.extend(other_files);

        for (user, user_config) in other_users {
            self.users.insert(user, user_config);
        }
    }
}
