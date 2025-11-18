#[macro_use]
extern crate serde_derive;

mod config;
mod disk_wrapper;

pub use crate::config::package::PackageConfig;
pub use crate::config::transaction_file::FileConfig;
pub use crate::config::Config;
use crate::disk_wrapper::DiskWrapper;

use anyhow::{anyhow, bail, Context, Result};
use pkg::Library;
use rand::{rngs::OsRng, TryRngCore};
use redoxfs::{Disk, DiskIo, FileSystem};
use termion::input::TermRead;

use std::{
    cell::RefCell,
    collections::BTreeMap,
    env, fs,
    io::{self, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process,
    rc::Rc,
    time::{SystemTime, UNIX_EPOCH},
};

pub struct DiskOption<'a> {
    pub bootloader_bios: &'a [u8],
    pub bootloader_efi: &'a [u8],
    pub password_opt: Option<&'a [u8]>,
    pub efi_partition_size: Option<u32>, //MiB
    pub skip_partitions: bool,
}

fn get_target() -> String {
    env::var("TARGET").unwrap_or(
        option_env!("TARGET").map_or("x86_64-unknown-redox".to_string(), |x| x.to_string()),
    )
}

/// Converts a password to a serialized argon2rs hash, understandable
/// by redox_users. If the password is blank, the hash is blank.
fn hash_password(password: &str) -> Result<String> {
    if !password.is_empty() {
        let salt = format!("{:X}", OsRng.try_next_u64()?);
        let config = argon2::Config::default();
        let hash = argon2::hash_encoded(password.as_bytes(), salt.as_bytes(), &config)?;
        Ok(hash)
    } else {
        Ok("".into())
    }
}

fn syscall_error(err: syscall::Error) -> io::Error {
    io::Error::from_raw_os_error(err.errno)
}

/// Returns a password collected from the user (plaintext)
fn prompt_password(prompt: &str, confirm_prompt: &str) -> Result<String> {
    let stdin = io::stdin();
    let mut stdin = stdin.lock();
    let stdout = io::stdout();
    let mut stdout = stdout.lock();

    print!("{}", prompt);
    let password = stdin.read_passwd(&mut stdout)?;

    print!("\n{}", confirm_prompt);
    let confirm_password = stdin.read_passwd(&mut stdout)?;

    // Note: Actually comparing two Option<String> values
    if confirm_password != password {
        bail!("passwords do not match");
    }
    Ok(password.unwrap_or("".to_string()))
}

fn install_local_pkgar(cookbook: &str, target: &str, packagename: &str, dest: &str) -> Result<()> {
    let head_path = get_head_path(packagename, dest);

    let public_path = format!("{cookbook}/build/id_ed25519.pub.toml",);
    let pkgar_path = format!("{cookbook}/repo/{target}/{packagename}.pkgar");

    let pkginfo_path = format!("{cookbook}/repo/{target}/{packagename}.toml");
    let pkginfo = pkg::Package::from_toml(&fs::read_to_string(pkginfo_path)?)?;

    if pkginfo.version != "" {
        pkgar::extract(&public_path, &pkgar_path, dest).unwrap();
        pkgar::split(&public_path, &pkgar_path, head_path, Option::<&str>::None).unwrap();
    }

    // Recursively install any runtime dependencies.
    for dep in pkginfo.depends.iter() {
        let depname = dep.as_str();
        if !get_head_path(depname, dest).exists() {
            println!("Installing runtime dependency for {packagename} from local repo: {depname}");
            install_local_pkgar(cookbook, target, depname, dest)?;
        }
    }

    Ok(())
}

fn get_head_path(packagename: &str, dest: &str) -> PathBuf {
    let head_path = PathBuf::from(format!("{dest}/pkg/packages/{packagename}.pkgar_head"));
    head_path
}

//TODO: error handling
fn install_packages(config: &Config, dest: &str, cookbook: Option<&str>) {
    let target = &get_target();

    let callback = pkg::callback::IndicatifCallback::new();
    let mut library = Library::new(dest, target, Rc::new(RefCell::new(callback))).unwrap();

    if let Some(cookbook) = cookbook {
        let mut local_packages = Vec::new();
        let mut remote_packages = Vec::new();
        let default_is_remote = config.general.repo_binary.unwrap_or(false);
        let dest_pkg = format!("{}/pkg/packages", dest);
        if !Path::new(&dest_pkg).is_dir() {
            fs::create_dir_all(&dest_pkg).unwrap();
        }

        for (packagename, package) in &config.packages {
            enum Rule {
                RemotePrebuilt,
                Build,
                Ignore,
            }

            let rule = match (default_is_remote, package) {
                (
                    true,
                    PackageConfig::Empty
                    | PackageConfig::Spec {
                        version: None,
                        git: None,
                        path: None,
                    },
                ) => Rule::RemotePrebuilt,
                (_, PackageConfig::Build(rule)) if rule == "binary" => Rule::RemotePrebuilt,
                (_, PackageConfig::Build(rule)) if rule == "ignore" => Rule::Ignore,
                _ => Rule::Build,
            };

            match rule {
                Rule::RemotePrebuilt => {
                    remote_packages.push(packagename);
                }
                Rule::Build => {
                    local_packages.push(packagename);
                }
                Rule::Ignore => {
                    // do nothing, not even logging it
                }
            }
        }

        // overrided packages source must be installed first
        if default_is_remote {
            install_local_packages(dest, target, local_packages, cookbook);
            install_remote_packages(dest, &mut library, remote_packages);
        } else {
            install_remote_packages(dest, &mut library, remote_packages);
            install_local_packages(dest, target, local_packages, cookbook);
        }
    } else {
        install_remote_packages(dest, &mut library, config.packages.keys().collect());
    }
}

fn install_local_packages(
    dest: &str,
    target: &String,
    local_packages: Vec<&String>,
    cookbook: &str,
) {
    for packagename in local_packages {
        if !get_head_path(packagename, dest).exists() {
            println!("Installing package from local repo: {}", packagename);
            install_local_pkgar(cookbook, target, packagename, dest).unwrap();
        }
    }
}

fn install_remote_packages(dest: &str, library: &mut Library, remote_packages: Vec<&String>) {
    for packagename in remote_packages {
        if !get_head_path(packagename, dest).exists() {
            println!("Installing package from remote: {packagename}");
            library
                .install(vec![pkg::PackageName::new(packagename).unwrap()])
                .unwrap();
        }
    }
    library.apply().unwrap();
}

pub fn install_dir<D: Disk>(
    config: Config,
    cookbook: Option<&str>,
    mut fs: FileSystem<D>,
) -> Result<()> {
    //let mut context = liner::Context::new();

    macro_rules! prompt {
        ($dst:expr, $def:expr, $($arg:tt)*) => {
            if config.general.prompt.unwrap_or(true) {
                Err(io::Error::new(
                    io::ErrorKind::Other,
                    "prompt not currently supported",
                ))
                // match unwrap_or_prompt($dst, &mut context, &format!($($arg)*)) {
                //     Ok(res) => if res.is_empty() {
                //         Ok($def)
                //     } else {
                //         Ok(res)
                //     },
                //     Err(err) => Err(err)
                // }
            } else {
                Ok($dst.unwrap_or($def))
            }
        };
    }

    for file in &config.files {
        if !file.postinstall {
            file.create(&mut fs)?;
        }
    }

    // TODO: how to migrate this to using the transaction API?
    // install_packages(&config, output_dir.to_str().unwrap(), cookbook);

    for file in &config.files {
        if file.postinstall {
            file.create(&mut fs)?;
        }
    }

    let mut passwd = String::new();
    let mut shadow = String::new();
    let mut next_uid = 1000;
    let mut next_gid = 1000;

    let mut groups = vec![];

    for (username, user) in config.users {
        // plaintext
        let password = if let Some(password) = user.password {
            password
        } else if config.general.prompt.unwrap_or(true) {
            prompt_password(
                &format!("{}: enter password: ", username),
                &format!("{}: confirm password: ", username),
            )?
        } else {
            String::new()
        };

        let uid = user.uid.unwrap_or(next_uid);

        if uid >= next_uid {
            next_uid = uid + 1;
        }

        let gid = user.gid.unwrap_or(next_gid);

        if gid >= next_gid {
            next_gid = gid + 1;
        }

        let name = prompt!(
            user.name,
            username.clone(),
            "{}: name (GECOS) [{}]: ",
            username,
            username
        )?;
        let home = prompt!(
            user.home,
            format!("/home/{}", username),
            "{}: home [/home/{}]: ",
            username,
            username
        )?;
        let shell = prompt!(
            user.shell,
            "/bin/ion".to_string(),
            "{}: shell [/bin/ion]: ",
            username
        )?;

        println!("Adding user {username}:");
        println!("\tPassword: {password}");
        println!("\tUID: {uid}");
        println!("\tGID: {gid}");
        println!("\tName: {name}");
        println!("\tHome: {home}");
        println!("\tShell: {shell}");

        FileConfig::new_directory(home.clone())
            .with_recursive_mod(0o777, uid, gid)
            .create(&mut fs)?;

        if uid >= 1000 {
            // Create XDG user dirs
            //TODO: move to some autostart program?
            for xdg_folder in &[
                "Desktop",
                "Documents",
                "Downloads",
                "Music",
                "Pictures",
                "Public",
                "Templates",
                "Videos",
                ".config",
                ".local",
                ".local/share",
                ".local/share/Trash",
                ".local/share/Trash/info",
            ] {
                FileConfig::new_directory(format!("{}/{}", home, xdg_folder))
                    .with_mod(0o0700, uid, gid)
                    .create(&mut fs)?;
            }

            FileConfig::new_file(
                format!("{}/.config/user-dirs.dirs", home),
                r#"# Produced by redox installer
XDG_DESKTOP_DIR="$HOME/Desktop"
XDG_DOCUMENTS_DIR="$HOME/Documents"
XDG_DOWNLOAD_DIR="$HOME/Downloads"
XDG_MUSIC_DIR="$HOME/Music"
XDG_PICTURES_DIR="$HOME/Pictures"
XDG_PUBLICSHARE_DIR="$HOME/Public"
XDG_TEMPLATES_DIR="$HOME/Templates"
XDG_VIDEOS_DIR="$HOME/Videos"
"#
                .to_string(),
            )
            .with_mod(0o0600, uid, gid)
            .create(&mut fs)?;
        }

        let password = hash_password(&password)?;

        passwd.push_str(&format!("{username};{uid};{gid};{name};{home};{shell}\n",));
        shadow.push_str(&format!("{username};{password}\n"));
        groups.push((username.clone(), gid, vec![username]));
    }

    for (group, group_config) in config.groups {
        // FIXME this assumes there is no overlap between auto-created groups for users
        // and explicitly specified groups.
        let gid = group_config.gid.unwrap_or(next_gid);

        if gid >= next_gid {
            next_gid = gid + 1;
        }

        groups.push((group, gid, group_config.members));
    }

    if !passwd.is_empty() {
        FileConfig::new_file("/etc/passwd".to_string(), passwd).create(&mut fs)?;
    }

    if !shadow.is_empty() {
        FileConfig::new_file("/etc/shadow".to_string(), shadow)
            .with_mod(0o0600, 0, 0)
            .create(&mut fs)?;
    }

    if !groups.is_empty() {
        let mut groups_data = String::new();

        for (name, gid, members) in groups {
            use std::fmt::Write;
            writeln!(groups_data, "{name};x;{gid};{}", members.join(",")).unwrap();

            println!("Adding group {name}:");
            println!("\tGID: {gid}");
            println!("\tMembers: {}", members.join(", "));
        }

        FileConfig::new_file("/etc/group".to_string(), groups_data)
            .with_mod(0o0600, 0, 0)
            .create(&mut fs)?;
    }

    Ok(())
}

pub fn with_redoxfs<D, T, F>(disk: D, password_opt: Option<&[u8]>, callback: F) -> Result<T>
where
    D: Disk + Send + 'static,
    F: FnOnce(FileSystem<D>) -> Result<T>,
{
    let ctime = SystemTime::now().duration_since(UNIX_EPOCH)?;
    let fs = FileSystem::create(disk, password_opt, ctime.as_secs(), ctime.subsec_nanos())
        .map_err(syscall_error)?;
    callback(fs)
}

pub fn fetch_bootloaders(
    config: &Config,
    cookbook: Option<&str>,
    live: bool,
) -> Result<(Vec<u8>, Vec<u8>)> {
    let bootloader_dir = format!("/tmp/redox_installer_bootloader_{}", process::id());

    if Path::new(&bootloader_dir).exists() {
        fs::remove_dir_all(&bootloader_dir)?;
    }

    fs::create_dir(&bootloader_dir)?;

    let mut bootloader_config = Config::default();
    bootloader_config.general = config.general.clone();
    // Ensure a pkgar remote is available
    crate::config::file::FileConfig {
        path: "/etc/pkg.d/50_redox".to_string(),
        data: "https://static.redox-os.org/pkg".to_string(),
        ..Default::default()
    }
    .create(&bootloader_dir)?;
    bootloader_config
        .packages
        .insert("bootloader".to_string(), PackageConfig::default());
    install_packages(&bootloader_config, &bootloader_dir, cookbook);

    let boot_dir = Path::new(&bootloader_dir).join("boot");
    let bios_path = boot_dir.join(if live {
        "bootloader-live.bios"
    } else {
        "bootloader.bios"
    });
    let efi_path = boot_dir.join(if live {
        "bootloader-live.efi"
    } else {
        "bootloader.efi"
    });

    let bios_data = if bios_path.exists() {
        fs::read(bios_path)?
    } else {
        Vec::new()
    };
    let efi_data = if efi_path.exists() {
        fs::read(efi_path)?
    } else {
        Vec::new()
    };

    fs::remove_dir_all(&bootloader_dir)?;

    Ok((bios_data, efi_data))
}

//TODO: make bootloaders use Option, dynamically create BIOS and EFI partitions
pub fn with_whole_disk<P, F, T>(disk_path: P, disk_option: &DiskOption, callback: F) -> Result<T>
where
    P: AsRef<Path>,
    F: FnOnce(FileSystem<DiskIo<fscommon::StreamSlice<DiskWrapper>>>) -> Result<T>,
{
    let target = get_target();

    let bootloader_efi_name = match target.as_str() {
        "aarch64-unknown-redox" => "BOOTAA64.EFI",
        "i586-unknown-redox" | "i686-unknown-redox" => "BOOTIA32.EFI",
        "x86_64-unknown-redox" => "BOOTX64.EFI",
        "riscv64gc-unknown-redox" => "BOOTRISCV64.EFI",
        _ => {
            bail!("target '{target}' not supported");
        }
    };
    // Open disk and read metadata
    eprintln!("Opening disk {}", disk_path.as_ref().display());
    let mut disk_file = DiskWrapper::open(disk_path.as_ref())?;
    let disk_size = disk_file.size();
    let block_size = disk_file.block_size() as u64;

    if disk_option.skip_partitions {
        return with_redoxfs(
            DiskIo(fscommon::StreamSlice::new(
                disk_file,
                0,
                disk_size.next_multiple_of(block_size),
            )?),
            disk_option.password_opt,
            callback,
        );
    }

    let gpt_block_size = match block_size {
        512 => gpt::disk::LogicalBlockSize::Lb512,
        _ => {
            // TODO: support (and test) other block sizes
            bail!("block size {block_size} not supported");
        }
    };

    // Calculate partition offsets
    let gpt_reserved = 34 * 512; // GPT always reserves 34 512-byte sectors
    let mibi = 1024 * 1024;

    // First megabyte of the disk is reserved for BIOS partition, wich includes GPT tables
    let bios_start = gpt_reserved / block_size;
    let bios_end = (mibi / block_size) - 1;

    // Second megabyte of the disk is reserved for EFI partition
    let efi_start = bios_end + 1;
    let efi_size = if let Some(size) = disk_option.efi_partition_size {
        size as u64
    } else {
        1
    };
    let efi_end = efi_start + (efi_size * mibi / block_size) - 1;

    // The rest of the disk is RedoxFS, reserving the GPT table mirror at the end of disk
    let redoxfs_start = efi_end + 1;
    let redoxfs_end = ((((disk_size - gpt_reserved) / mibi) * mibi) / block_size) - 1;

    // Format and install BIOS partition
    {
        // Write BIOS bootloader to disk
        eprintln!(
            "Write bootloader with size {:#x}",
            disk_option.bootloader_bios.len()
        );
        disk_file.seek(SeekFrom::Start(0))?;
        disk_file.write_all(&disk_option.bootloader_bios)?;

        // Replace MBR tables with protective MBR
        // TODO: div_ceil
        let mbr_blocks = ((disk_size + block_size - 1) / block_size) - 1;
        eprintln!("Writing protective MBR with disk blocks {mbr_blocks:#x}");
        gpt::mbr::ProtectiveMBR::with_lb_size(mbr_blocks as u32)
            .update_conservative(&mut disk_file)?;

        // Open disk, mark it as not initialized
        let mut gpt_disk = gpt::GptConfig::new()
            .initialized(false)
            .writable(true)
            .logical_block_size(gpt_block_size)
            .create_from_device(Box::new(&mut disk_file), None)?;

        // Add BIOS boot partition
        let mut partitions = BTreeMap::new();
        let mut partition_id = 1;
        partitions.insert(
            partition_id,
            gpt::partition::Partition {
                part_type_guid: gpt::partition_types::BIOS,
                part_guid: uuid::Uuid::new_v4(),
                first_lba: bios_start,
                last_lba: bios_end,
                flags: 0, // TODO
                name: "BIOS".to_string(),
            },
        );
        partition_id += 1;

        // Add EFI boot partition
        partitions.insert(
            partition_id,
            gpt::partition::Partition {
                part_type_guid: gpt::partition_types::EFI,
                part_guid: uuid::Uuid::new_v4(),
                first_lba: efi_start,
                last_lba: efi_end,
                flags: 0, // TODO
                name: "EFI".to_string(),
            },
        );
        partition_id += 1;

        // Add RedoxFS partition
        partitions.insert(
            partition_id,
            gpt::partition::Partition {
                //TODO: Use REDOX_REDOXFS type (needs GPT crate changes)
                part_type_guid: gpt::partition_types::LINUX_FS,
                part_guid: uuid::Uuid::new_v4(),
                first_lba: redoxfs_start,
                last_lba: redoxfs_end,
                flags: 0,
                name: "REDOX".to_string(),
            },
        );

        eprintln!("Writing GPT tables: {partitions:#?}");

        // Initialize GPT table
        gpt_disk.update_partitions(partitions)?;

        // Write partition layout, returning disk file
        gpt_disk.write()?;
    }

    // Format and install EFI partition
    {
        let disk_efi_start = efi_start * block_size;
        let disk_efi_end = (efi_end + 1) * block_size;
        let mut disk_efi =
            fscommon::StreamSlice::new(&mut disk_file, disk_efi_start, disk_efi_end)?;

        eprintln!(
            "Formatting EFI partition with size {:#x}",
            disk_efi_end - disk_efi_start
        );
        fatfs::format_volume(&mut disk_efi, fatfs::FormatVolumeOptions::new())?;

        eprintln!("Opening EFI partition");
        let fs = fatfs::FileSystem::new(&mut disk_efi, fatfs::FsOptions::new())?;

        eprintln!("Creating EFI directory");
        let root_dir = fs.root_dir();
        root_dir.create_dir("EFI")?;

        eprintln!("Creating EFI/BOOT directory");
        let efi_dir = root_dir.open_dir("EFI")?;
        efi_dir.create_dir("BOOT")?;

        eprintln!(
            "Writing EFI/BOOT/{} file with size {:#x}",
            bootloader_efi_name,
            disk_option.bootloader_efi.len()
        );
        let boot_dir = efi_dir.open_dir("BOOT")?;
        let mut file = boot_dir.create_file(bootloader_efi_name)?;
        file.truncate()?;
        file.write_all(&disk_option.bootloader_efi)?;
    }

    // Format and install RedoxFS partition
    eprintln!(
        "Installing to RedoxFS partition with size {:#x}",
        (redoxfs_end - redoxfs_start) * block_size
    );
    let disk_redoxfs = DiskIo(fscommon::StreamSlice::new(
        disk_file,
        redoxfs_start * block_size,
        (redoxfs_end + 1) * block_size,
    )?);
    with_redoxfs(disk_redoxfs, disk_option.password_opt, callback)
}

#[cfg(not(target_os = "redox"))]
pub fn try_fast_install<D: redoxfs::Disk, F: FnMut(u64, u64)>(
    _fs: &mut redoxfs::FileSystem<D>,
    _progress: F,
) -> Result<bool> {
    Ok(false)
}

/// Try fast install using live disk memory
#[cfg(target_os = "redox")]
pub fn try_fast_install<D: redoxfs::Disk, F: FnMut(u64, u64)>(
    fs: &mut redoxfs::FileSystem<D>,
    mut progress: F,
) -> Result<bool> {
    use libredox::{call::MmapArgs, flag};
    use std::os::fd::AsRawFd;
    use syscall::PAGE_SIZE;

    let phys = env::var("DISK_LIVE_ADDR")
        .ok()
        .and_then(|x| usize::from_str_radix(&x, 16).ok())
        .unwrap_or(0);
    let size = env::var("DISK_LIVE_SIZE")
        .ok()
        .and_then(|x| usize::from_str_radix(&x, 16).ok())
        .unwrap_or(0);
    if phys == 0 || size == 0 {
        return Ok(false);
    }

    let start = (phys / PAGE_SIZE) * PAGE_SIZE;
    let end = phys
        .checked_add(size)
        .context("phys + size overflow")?
        .next_multiple_of(PAGE_SIZE);
    let size = end - start;

    let original = unsafe {
        //TODO: unmap this memory
        let file = fs::File::open("/scheme/memory/physical")?;
        let base = libredox::call::mmap(MmapArgs {
            fd: file.as_raw_fd() as usize,
            addr: core::ptr::null_mut(),
            offset: start as u64,
            length: size,
            prot: flag::PROT_READ,
            flags: flag::MAP_SHARED,
        })
        .map_err(|err| anyhow!("failed to mmap livedisk: {}", err))?;

        std::slice::from_raw_parts(base as *const u8, size)
    };

    struct DiskLive {
        original: &'static [u8],
    }

    impl redoxfs::Disk for DiskLive {
        unsafe fn read_at(&mut self, block: u64, buffer: &mut [u8]) -> syscall::Result<usize> {
            let offset = (block * redoxfs::BLOCK_SIZE) as usize;
            if offset + buffer.len() > self.original.len() {
                return Err(syscall::Error::new(syscall::EINVAL));
            }
            buffer.copy_from_slice(&self.original[offset..offset + buffer.len()]);
            Ok(buffer.len())
        }

        unsafe fn write_at(&mut self, _block: u64, _buffer: &[u8]) -> syscall::Result<usize> {
            Err(syscall::Error::new(syscall::EINVAL))
        }

        fn size(&mut self) -> syscall::Result<u64> {
            Ok(self.original.len() as u64)
        }
    }

    let mut fs_old = redoxfs::FileSystem::open(DiskLive { original }, None, None, false)?;
    let size_old = fs_old.header.size();
    let free_old = fs_old.allocator().free() * redoxfs::BLOCK_SIZE;
    let used_old = size_old - free_old;
    redoxfs::clone(&mut fs_old, fs, move |used| {
        progress(used, used_old);
    })?;

    Ok(true)
}

fn install_inner(
    config: Config,
    output: &Path,
    cookbook: Option<&str>,
    live: bool,
    write_bootloader: Option<&str>,
) -> Result<()> {
    println!("Install {config:#?} to {}", output.display());

    if output.is_dir() {
        // TODO: will this option be needed if migrated to using the transaction API?
        todo!()
    } else {
        let (bootloader_bios, bootloader_efi) = fetch_bootloaders(&config, cookbook, live)?;
        if let Some(write_bootloader) = write_bootloader {
            std::fs::write(write_bootloader, &bootloader_efi).unwrap();
        }
        let disk_option = DiskOption {
            bootloader_bios: &bootloader_bios,
            bootloader_efi: &bootloader_efi,
            password_opt: None,
            efi_partition_size: config.general.efi_partition_size,
            skip_partitions: config.general.skip_partitions.unwrap_or(false),
        };
        with_whole_disk(output, &disk_option, move |fs| {
            install_dir(config, cookbook, fs)
        })
    }
}
pub fn install(
    config: Config,
    output: impl AsRef<Path>,
    cookbook: Option<&str>,
    live: bool,
    write_bootloader: Option<&str>,
) -> Result<()> {
    install_inner(config, output.as_ref(), cookbook, live, write_bootloader)
}
