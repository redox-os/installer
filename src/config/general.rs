#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct GeneralConfig {
    pub prompt: Option<bool>,
    // Allow config to specify cookbook recipe or binary package as default
    pub repo_binary: Option<bool>,
    pub filesystem_size: Option<u32>, //MiB
    pub efi_partition_size: Option<u32>, //MiB
}

impl GeneralConfig {
    pub(super) fn merge(&mut self, other: GeneralConfig) {
        self.prompt = other.prompt.or(self.prompt);
        self.repo_binary = other.repo_binary.or(self.repo_binary);
        self.filesystem_size = other.filesystem_size.or(self.filesystem_size);
        self.efi_partition_size = other.efi_partition_size.or(self.efi_partition_size);
    }
}
