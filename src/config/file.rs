#[derive(Debug, Default, Deserialize)]
pub struct FileConfig {
    pub path: String,
    pub data: String,
    #[serde(default)]
    pub symlink: bool
}
