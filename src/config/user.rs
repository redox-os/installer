use std::fmt::Display;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct UserConfig {
    pub password: Option<String>,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    pub name: Option<String>,
    pub home: Option<String>,
    pub shell: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct GroupConfig {
    pub gid: Option<u32>,
    // FIXME move this to the UserConfig struct as extra_groups
    pub members: Vec<String>,
}

impl Display for UserConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(uid) = &self.uid {
            write!(f, " uid={}", uid)?;
        }
        if let Some(gid) = &self.gid {
            write!(f, " gid={}", gid)?;
        }
        if let Some(name) = &self.name {
            write!(f, " name={}", name)?;
        }
        if let Some(home) = &self.home {
            write!(f, " home={}", home)?;
        }
        if let Some(shell) = &self.shell {
            write!(f, " shell={}", shell)?;
        }
        if self.password.as_ref().is_some_and(|s| !s.is_empty()) {
            write!(f, " password=yes")?;
        }

        Ok(())
    }
}
