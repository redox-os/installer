#[derive(Debug, Default, Deserialize)]
pub struct PackageConfig {
    pub version: String,
    pub git: String,
    pub path: String,
}
