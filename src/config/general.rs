#[derive(Clone, Debug, Default, Deserialize)]
pub struct GeneralConfig {
    pub prompt: bool,
    // Allow filesystem config to select recipe or package
    #[serde(skip)]
    pub cooking: Option<bool>,
}
