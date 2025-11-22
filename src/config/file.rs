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
