#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct GeneralConfig {
    pub prompt: bool,
    // Allow config to specify cookbook recipe or binary package as default
    pub repo_binary: Option<bool>,
    pub efi_partition_size: Option<u32>, //MiB
}
