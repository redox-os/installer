#[derive(Clone, Debug, Default, Deserialize)]
pub struct GeneralConfig {
    pub prompt: bool,
    // Allow config to specify cookbook recipe or binary package as default
    pub repo_binary: Option<bool>,
}
