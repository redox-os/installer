use std::fmt::Display;

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct FileConfig {
    pub path: String,
    pub data: String,
    #[serde(default)]
    pub symlink: bool,
    #[serde(default)]
    pub directory: bool,
    pub mode: Option<u32>,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    #[serde(default)]
    pub recursive_chown: bool,
    #[serde(default)]
    pub postinstall: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub append: bool,
}

impl FileConfig {
    pub fn new_file(path: String, data: String) -> FileConfig {
        FileConfig {
            path,
            data,
            ..Default::default()
        }
    }

    pub fn new_directory(path: String) -> FileConfig {
        FileConfig {
            path,
            data: String::new(),
            directory: true,
            ..Default::default()
        }
    }

    pub fn with_mod(&mut self, mode: u32, uid: u32, gid: u32) -> &mut FileConfig {
        self.mode = Some(mode);
        self.uid = Some(uid);
        self.gid = Some(gid);
        self
    }

    pub fn with_recursive_mod(&mut self, mode: u32, uid: u32, gid: u32) -> &mut FileConfig {
        self.with_mod(mode, uid, gid);
        self.recursive_chown = true;
        self
    }
}

impl Display for FileConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.path)?;
        if self.symlink {
            write!(f, " -> {}", self.data)?;
        } else if self.directory {
            write!(f, " type=dir")?;
            if self.recursive_chown {
                write!(f, " chown=yes")?;
            }
        } else {
            write!(f, " size={}", format_bytes(self.data.len() as u64))?;
            if self.postinstall {
                write!(f, "!")?;
            }
            if self.append {
                write!(f, " append=yes")?;
            }
        }
        if let Some(uid) = self.uid {
            write!(f, " uid={}", uid)?;
        }
        if let Some(uid) = self.uid {
            write!(f, " gid={}", uid)?;
        }
        if let Some(mode) = self.mode {
            write!(f, " mode={:3o}", mode)?;
        }
        Ok(())
    }
}

/// Convert bytes into readable string
pub fn format_bytes(len: u64) -> String {
    const GB: u64 = 1024 * 1024 * 1024;
    const MB: u64 = 1024 * 1024;
    const KB: u64 = 1024;

    if len > GB {
        format_bytes_inner(len, GB, "GB")
    } else if len > MB {
        format_bytes_inner(len, MB, "MB")
    } else if len > KB {
        format_bytes_inner(len, KB, "KB")
    } else {
        format!("{len} B")
    }
}

fn format_bytes_inner(len: u64, divisor: u64, suffix: &'static str) -> String {
    use std::fmt::Write;
    let mut s = format!("{}", len / divisor);
    if s.len() == 1 {
        let _ = write!(s, ".{:02}", (len % divisor) / (divisor / 100));
    } else if s.len() == 2 {
        let _ = write!(s, ".{:01}", (len % divisor) / (divisor / 10));
    }

    let _ = write!(s, " {suffix}");
    s
}
