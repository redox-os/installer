#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub enum PackageConfig {
    Empty,
    Build(String),
    Spec {
        version: Option<String>,
        git: Option<String>,
        path: Option<String>,
    }
}

impl Default for PackageConfig {
    fn default() -> Self {
        Self::Empty
    }
}
