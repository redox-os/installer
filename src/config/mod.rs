use std::collections::BTreeMap;
use std::fmt::{self, Write};

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

impl fmt::Display for Config {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        writeln!(f, "{} packages:", self.packages.len())?;
        for (name, conf) in &self.packages {
            let f = &mut indenter::indented(f);
            if let Some(ver) = &conf.version {
                write!(f, "{} v{}", name, ver)?;
            } else {
                f.write_str(name)?;
            }
            writeln!(f)?;
        }

        writeln!(f, "{} files:", self.files.len())?;
        for file in &self.files {
            let f = &mut indenter::indented(f);
            if file.symlink {
                writeln!(f, "symlink {} -> {}", file.path, file.data)?;
            } else {
                let data = file.data.trim();
                let trailing_slash = if file.directory { "/" } else { "" };
                let mode = file
                    .mode
                    .map_or_else(String::new, |m| format!(" ({:#o})", m));
                let colon = if data.is_empty() { "" } else { ":" };
                write!(f, "{}{}{}{}", file.path, trailing_slash, mode, colon)?;
                if data.contains('\n') {
                    writeln!(f)?;
                    let f = &mut indenter::indented(f);
                    writeln!(f, "{}", data)?;
                } else {
                    writeln!(f, " {}", data)?;
                }
            }
        }

        writeln!(f, "{} users:", self.users.len())?;
        for (name, user) in &self.users {
            let f = &mut indenter::indented(f);
            writeln!(f, "{}:", name)?;

            let f = &mut indenter::indented(f);

            writeln!(f, "password: {:?}", user.password)?;
            if let Some(home) = &user.home {
                writeln!(f, "home: {}", home)?;
            }
            if let Some(shell) = &user.shell {
                writeln!(f, "shell: {}", shell)?;
            }
            if let Some(uid) = user.uid {
                writeln!(f, "uid: {}", uid)?;
            }
            if let Some(gid) = user.gid {
                writeln!(f, "gid: {}", gid)?;
            }
        }

        Ok(())
    }
}
