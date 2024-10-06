#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum PackageConfig {
    Empty,
    Build(String),

    // TODO: Sum type
    Spec {
        version: Option<String>,
        git: Option<String>,
        path: Option<String>,
        pkg_path: Option<String>,
    },
}

impl Default for PackageConfig {
    fn default() -> Self {
        Self::Empty
    }
}
