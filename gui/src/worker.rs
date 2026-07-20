use crate::{InstallConfig, InstallConfigKind, Message};
use anyhow::format_err;
use futures_channel::mpsc;
use pkgar::{ext::EntryExt, PackageHead};
use pkgar_core::PackageSrc;
use pkgar_keys::PublicKeyFile;
use redox_installer::{try_fast_install, with_redoxfs_mount, with_whole_disk, Config, DiskOption};
use std::{
    ffi::OsStr,
    fs::{self, File},
    io::{self, Read, Write},
    os::unix::fs::{symlink, MetadataExt, OpenOptionsExt},
    path::Path,
    sync::Arc,
};

pub fn copy_file(src: &Path, dest: &Path, buf: &mut [u8]) -> anyhow::Result<()> {
    if let Some(parent) = dest.parent() {
        // Parent may be a symlink
        if !parent.is_symlink() {
            match fs::create_dir_all(&parent) {
                Ok(()) => (),
                Err(err) => {
                    return Err(format_err!(
                        "failed to create directory {}: {}",
                        parent.display(),
                        err
                    ));
                }
            }
        }
    }

    let metadata = match fs::symlink_metadata(&src) {
        Ok(ok) => ok,
        Err(err) => {
            return Err(format_err!(
                "failed to read metadata of {}: {}",
                src.display(),
                err
            ));
        }
    };

    if metadata.file_type().is_symlink() {
        let real_src = match fs::read_link(&src) {
            Ok(ok) => ok,
            Err(err) => {
                return Err(format_err!(
                    "failed to read link {}: {}",
                    src.display(),
                    err
                ));
            }
        };

        match symlink(&real_src, &dest) {
            Ok(()) => (),
            Err(err) => {
                return Err(format_err!(
                    "failed to copy link {} ({}) to {}: {}",
                    src.display(),
                    real_src.display(),
                    dest.display(),
                    err
                ));
            }
        }
    } else {
        let mut src_file = match fs::File::open(&src) {
            Ok(ok) => ok,
            Err(err) => {
                return Err(format_err!(
                    "failed to open file {}: {}",
                    src.display(),
                    err
                ));
            }
        };

        let mut dest_file = match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(metadata.mode())
            .open(&dest)
        {
            Ok(ok) => ok,
            Err(err) => {
                return Err(format_err!(
                    "failed to create file {}: {}",
                    dest.display(),
                    err
                ));
            }
        };

        loop {
            let count = match src_file.read(buf) {
                Ok(ok) => ok,
                Err(err) => {
                    return Err(format_err!(
                        "failed to read file {}: {}",
                        src.display(),
                        err
                    ));
                }
            };

            if count == 0 {
                break;
            }

            match dest_file.write_all(&buf[..count]) {
                Ok(()) => (),
                Err(err) => {
                    return Err(format_err!(
                        "failed to write file {}: {}",
                        dest.display(),
                        err
                    ));
                }
            }
        }
    }

    Ok(())
}

pub fn package_files(
    root_path: &Path,
    config: &mut Config,
    files: &mut Vec<String>,
) -> Result<(), anyhow::Error> {
    //TODO: Remove packages from config where all files are located (and have valid shasum?)
    config.packages.clear();

    let pkey_path = "pkg/id_ed25519.pub.toml";
    let pkey = PublicKeyFile::open(&root_path.join(pkey_path))?.pkey;
    files.push(pkey_path.to_string());

    for item_res in fs::read_dir(&root_path.join("pkg"))? {
        let item = item_res?;
        let pkg_path = item.path();
        if pkg_path.extension() == Some(OsStr::new("pkgar_head")) {
            let mut pkg = PackageHead::new(&pkg_path, &root_path, &pkey)?;
            for entry in pkg.read_entries()? {
                files.push(entry.check_path()?.to_str().unwrap().to_string());
            }
            files.push(
                pkg_path
                    .strip_prefix(root_path)
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_string(),
            );
        }
    }

    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetConfig {
    Disk((String, u64)),
    Partition((String, u64)),
    Image {
        path: String,
        size_mb: u64,
        skip_partition: bool,
    },
}

impl TargetConfig {
    pub fn is_skip_partition(&self) -> bool {
        match self {
            TargetConfig::Disk(_) => false,
            TargetConfig::Partition(_) => true,
            TargetConfig::Image {
                skip_partition: partition_only,
                ..
            } => *partition_only,
        }
    }
    pub fn install_path(&self) -> &str {
        match self {
            TargetConfig::Disk((s, _)) => s.as_str(),
            TargetConfig::Partition((s, _)) => s.as_str(),
            TargetConfig::Image { path, .. } => path.as_str(),
        }
    }
    pub fn install_size_mb(&self) -> u64 {
        match self {
            TargetConfig::Disk((_, s)) => *s / (1024 * 1024),
            TargetConfig::Partition((_, s)) => *s / (1024 * 1024),
            TargetConfig::Image { size_mb, .. } => *size_mb,
        }
    }
}

pub(crate) fn install<F: FnMut(Message)>(target: TargetConfig, profile: InstallConfig, mut f: F) {
    let start = std::time::Instant::now();
    let password_opt = profile.password_opt.filter(|s| !s.is_empty());
    let mut config: Config = match profile.kind {
        InstallConfigKind::Desktop => toml::from_str(include_str!("../../config/desktop.toml"))
            .expect("Cannot parse compiled desktop.toml"),
        InstallConfigKind::Server => toml::from_str(include_str!("../../config/server.toml"))
            .expect("Cannot parse compiled server.toml"),
        #[cfg(target_os = "redox")]
        InstallConfigKind::Clone => {
            install_clone_disk(target, profile.live_disk, password_opt, f);
            return;
        }
    };

    config.general.live_disk = Some(profile.live_disk);
    config.general.encrypt_disk = password_opt;
    config.general.skip_partitions = Some(target.is_skip_partition());
    config.general.filesystem_size = Some(target.install_size_mb() as u32);

    macro_rules! message {
        ($($arg:tt)*) => {{
            eprintln!($($arg)*);
            f(Message::Install(
                0,
                format!($($arg)*)
            ));
        }}
    }
    message!("Installation progress is in terminal");

    // TODO: Show progress
    match redox_installer::install(config, Path::new(target.install_path())) {
        Ok(_) => f(Message::Success(format!(
            "Finished installing in {:?}",
            start.elapsed()
        ))),
        Err(e) => f(Message::Error(format!("Failed installing: {:?}", e))),
    }
}

#[allow(unused)]
pub(crate) fn install_clone_disk<F: FnMut(Message)>(
    target: TargetConfig,
    live: bool,
    password_opt: Option<String>,
    mut f: F,
) {
    let start = std::time::Instant::now();

    let mut progress = 0;

    macro_rules! message {
        ($($arg:tt)*) => {{
            eprintln!($($arg)*);
            f(Message::Install(
                progress,
                format!($($arg)*)
            ));
        }}
    }

    let root_path = Path::new("/scheme/file/");

    message!("Loading bootloader");
    let bootloader_bios = {
        let path = root_path.join("boot").join("bootloader.bios");
        if path.exists() {
            match fs::read(&path) {
                Ok(ok) => ok,
                Err(err) => {
                    f(Message::Error(format!(
                        "{}: failed to read: {}",
                        path.display(),
                        err
                    )));
                    return;
                }
            }
        } else {
            Vec::new()
        }
    };

    message!("Loading bootloader.efi");
    let bootloader_efi = {
        let path = root_path.join("boot").join("bootloader.efi");
        if path.exists() {
            match fs::read(&path) {
                Ok(ok) => ok,
                Err(err) => {
                    f(Message::Error(format!(
                        "{}: failed to read: {}",
                        path.display(),
                        err
                    )));
                    return;
                }
            }
        } else {
            Vec::new()
        }
    };

    message!("Formatting disk");
    let disk_option = DiskOption {
        bootloader_bios: &bootloader_bios,
        bootloader_efi: &bootloader_efi,
        password_opt: password_opt.as_ref().map(|x| x.as_bytes()),
        efi_partition_size: None,
        skip_partitions: target.is_skip_partition(),
    };

    let disk_path = target.install_path();

    if let (TargetConfig::Image { path, size_mb, .. }) = &target {
        let file = match File::create_new(path) {
            Ok(file) => file,
            Err(err) => {
                message!("{}: failed to create file: {}", path, err);
                return;
            }
        };
        if let Err(err) = file.set_len(*size_mb * 1024 * 1024) {
            message!("{}: failed to truncate: {}", path, err);
            return;
        }
    }

    let res = with_whole_disk(disk_path, &disk_option, |mut fs| -> anyhow::Result<()> {
        // Fast install method via filesystem clone
        let mut last_progress = 0;
        if try_fast_install(&mut fs, |used, used_old| {
            progress = ((used * 100) / used_old) as usize;
            if progress != last_progress {
                message!(
                    "{}%: {} MB/{} MB",
                    progress,
                    used / 1000 / 1000,
                    used_old / 1000 / 1000
                );
                last_progress = progress;
            }
        })? {
            progress = 100;
            message!("Finished installing using fast mode");
            return Ok(());
        }

        with_redoxfs_mount(fs, None, |mount_path: &Path| -> anyhow::Result<()> {
            message!("Loading filesystem.toml");
            let mut config: Config = {
                let path = root_path.join("filesystem.toml");
                match fs::read_to_string(&path) {
                    Ok(config_data) => match toml::from_str(&config_data) {
                        Ok(config) => config,
                        Err(err) => {
                            return Err(format_err!(
                                "{}: failed to decode: {}",
                                path.display(),
                                err
                            ));
                        }
                    },
                    Err(err) => {
                        return Err(format_err!("{}: failed to read: {}", path.display(), err));
                    }
                }
            };

            // Copy filesystem.toml, which is not packaged
            let mut files = vec!["filesystem.toml".to_string()];

            // Copy files from locally installed packages
            message!("Loading package files");
            if let Err(err) = package_files(&root_path, &mut config, &mut files) {
                return Err(format_err!("failed to read package files: {}", err));
            }

            // Sort and remove duplicates
            files.sort();
            files.dedup();

            // Perform config install (after packages have been converted to files)
            message!("Configuring system");
            let cookbook: Option<&'static str> = None;
            redox_installer::install_dir(config, mount_path, cookbook)
                .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;

            // Install files
            let mut buf = vec![0; 4096 * 1024];
            for (i, name) in files.iter().enumerate() {
                progress = (i * 100) / files.len();
                message!("Copy {} [{}/{}]", name, i, files.len());

                let src = root_path.join(name);
                let dest = mount_path.join(name);
                copy_file(&src, &dest, &mut buf)?;
            }

            progress = 100;
            message!("Finished installing, unmounting filesystem");
            Ok(())
        })
    });

    match res {
        Ok(()) => {
            f(Message::Success(format!(
                "Finished installing in {:?}, ready to reboot",
                start.elapsed()
            )));
        }
        Err(err) => {
            f(Message::Error(format!("Failed to install: {}", err)));
        }
    }

    f(Message::Success(format!(
        "Finished installing in {:?}",
        start.elapsed()
    )));
}
