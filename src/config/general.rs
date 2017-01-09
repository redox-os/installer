#[derive(Debug, Default, Deserialize)]
pub struct GeneralConfig {
    pub prompt: bool,
    pub sysroot: Option<String>
}
