#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct GeneralConfig {
    /// Specify a path where cookbook exists, all packages will be installed locally
    pub cookbook: Option<String>,
    /// Allow prompts for missing information such as user password, true by default
    pub prompt: Option<bool>,
    /// Allow config to specify cookbook recipe or binary package as default
    /// note: Not read by installer anymore, exists only for legacy reasons
    pub repo_binary: Option<bool>,
    /// Total filesystem size in MB
    pub filesystem_size: Option<u32>,
    /// EFI partition size in MB, default to 2MB
    pub efi_partition_size: Option<u32>,
    /// Skip disk partitioning, assume whole disk is a partition
    pub skip_partitions: Option<bool>,
    /// Set a plain text password to encrypt the disk, or empty string to prompt
    pub encrypt_disk: Option<String>,
    /// Use live disk for bootloader config, default is false
    pub live_disk: Option<bool>,
    /// If set, write bootloader disk into this path
    pub write_bootloader: Option<String>,
    /// Use AR to write files instead of FUSE-based mount
    /// (bypasses FUSE, but slower and requires namespaced context such as "podman unshare")
    pub no_mount: Option<bool>,
}

impl GeneralConfig {
    /// Merge two config, "other" is more dominant
    pub(super) fn merge(&mut self, other: GeneralConfig) {
        if let Some(cookbook) = other.cookbook {
            self.cookbook = Some(cookbook);
        }
        self.prompt = other.prompt.or(self.prompt);
        self.repo_binary = other.repo_binary.or(self.repo_binary);
        self.filesystem_size = other.filesystem_size.or(self.filesystem_size);
        self.efi_partition_size = other.efi_partition_size.or(self.efi_partition_size);
        self.skip_partitions = other.skip_partitions.or(self.skip_partitions);
        if let Some(encrypt_disk) = other.encrypt_disk {
            self.encrypt_disk = Some(encrypt_disk);
        }
        self.live_disk = other.live_disk.or(self.live_disk);
        if let Some(write_bootloader) = other.write_bootloader {
            self.write_bootloader = Some(write_bootloader);
        }
        self.no_mount = other.no_mount.or(self.no_mount);
    }
}
