#[macro_use]
extern crate serde_derive;
extern crate argon2;
extern crate libc;
extern crate liner;
#[macro_use]
extern crate failure;
extern crate pkgutils;
extern crate rand;
extern crate redoxfs;
extern crate syscall;
extern crate termion;

mod config;
mod disk_wrapper;

pub use config::Config;
pub use config::file::FileConfig;
pub use config::package::PackageConfig;
use disk_wrapper::DiskWrapper;

use failure::{Error, err_msg};
use rand::{RngCore, rngs::OsRng};
use redoxfs::{Disk, DiskIo, FileSystem};
use termion::input::TermRead;
use pkgutils::{Repo, Package};

use std::{
    collections::BTreeMap,
    env,
    fs,
    io::{self, Seek, SeekFrom, Write},
    path::Path,
    process::{Command},
    sync::mpsc::channel,
    time::{SystemTime, UNIX_EPOCH},
    thread,
};

pub(crate) type Result<T> = std::result::Result<T, Error>;

const REMOTE: &'static str = "https://static.redox-os.org/pkg";

fn get_target() -> String {
    env::var("TARGET").unwrap_or(
        option_env!("TARGET").map_or(
            "x86_64-unknown-redox".to_string(),
            |x| x.to_string()
        )
    )
}

/// Converts a password to a serialized argon2rs hash, understandable
/// by redox_users. If the password is blank, the hash is blank.
fn hash_password(password: &str) -> Result<String> {
    if password != "" {
        let salt = format!("{:X}", OsRng.next_u64());
        let config = argon2::Config::default();
        let hash = argon2::hash_encoded(password.as_bytes(), salt.as_bytes(), &config)?;
        Ok(hash)
    } else {
        Ok("".to_string())
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
    if confirm_password == password {
        Ok(password.unwrap_or("".to_string()))
    } else {
        Err(err_msg("passwords do not match"))
    }
}

//TODO: error handling
fn install_packages<S: AsRef<str>>(config: &Config, dest: &str, cookbook: Option<S>) {
    let target = &get_target();

    let mut repo = Repo::new(target);
    repo.add_remote(REMOTE);

    if let Some(cookbook) = cookbook {
        let dest_pkg = format!("{}/pkg", dest);
        if ! Path::new(&dest_pkg).exists() {
            fs::create_dir(&dest_pkg).unwrap();
        }

        for (packagename, _package) in &config.packages {
            println!("Installing package {}", packagename);
            let pkgar_path = format!("{}/{}/repo/{}/{}.pkgar",
                                     env::current_dir().unwrap().to_string_lossy(),
                                     cookbook.as_ref(), target, packagename);
            if Path::new(&pkgar_path).exists() {
                let public_path = format!("{}/{}/build/id_ed25519.pub.toml",
                                          env::current_dir().unwrap().to_string_lossy(),
                                          cookbook.as_ref());
                pkgar::extract(&public_path, &pkgar_path, dest).unwrap();

                let head_path = format!("{}/{}.pkgar_head", dest_pkg, packagename);
                pkgar::split(&public_path, &pkgar_path, &head_path, Option::<&str>::None).unwrap();
            } else {
                let path = format!("{}/{}/repo/{}/{}.tar.gz",
                                   env::current_dir().unwrap().to_string_lossy(),
                                   cookbook.as_ref(), target, packagename);
                Package::from_path(&path).unwrap().install(dest).unwrap();
            }
        }
    } else {
        for (packagename, _package) in &config.packages {
            println!("Installing package {}", packagename);
            repo.fetch(&packagename).unwrap().install(dest).unwrap();
        }
    }
}

pub fn install_dir<P: AsRef<Path>, S: AsRef<str>>(config: Config, output_dir: P, cookbook: Option<S>) -> Result<()> {
    //let mut context = liner::Context::new();

    macro_rules! prompt {
        ($dst:expr, $def:expr, $($arg:tt)*) => (if config.general.prompt {
            Err(io::Error::new(
                io::ErrorKind::Other,
                "prompt not currently supported"
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
        })
    }

    let output_dir = output_dir.as_ref();

    let output_dir = output_dir.to_owned();

    install_packages(&config, output_dir.to_str().unwrap(), cookbook);

    for file in config.files {
        file.create(&output_dir)?;
    }

    let mut passwd = String::new();
    let mut shadow = String::new();
    let mut next_uid = 1000;

    for (username, user) in config.users {
        // plaintext
        let password = if let Some(password) = user.password {
            password
        } else if config.general.prompt {
            prompt_password(
                &format!("{}: enter password: ", username),
                &format!("{}: confirm password: ", username))?
        } else {
            String::new()
        };

        let uid = user.uid.unwrap_or(next_uid);

        if uid >= next_uid {
            next_uid = uid + 1;
        }

        let gid = user.gid.unwrap_or(uid);

        let name = prompt!(user.name, username.clone(), "{}: name (GECOS) [{}]: ", username, username)?;
        let home = prompt!(user.home, format!("/home/{}", username), "{}: home [/home/{}]: ", username, username)?;
        let shell = prompt!(user.shell, "/bin/ion".to_string(), "{}: shell [/bin/ion]: ", username)?;

        println!("Adding user {}:", username);
        println!("\tPassword: {}", password);
        println!("\tUID: {}", uid);
        println!("\tGID: {}", gid);
        println!("\tName: {}", name);
        println!("\tHome: {}", home);
        println!("\tShell: {}", shell);

        FileConfig {
            path: home.clone(),
            data: String::new(),
            symlink: false,
            directory: true,
            mode: Some(0o0700),
            uid: Some(uid),
            gid: Some(gid),
            recursive_chown: true,
        }.create(&output_dir)?;

        let password = hash_password(&password)?;

        passwd.push_str(&format!("{};{};{};{};file:{};file:{}\n", username, uid, gid, name, home, shell));
        shadow.push_str(&format!("{};{}\n", username, password));
    }

    if !passwd.is_empty() {
        FileConfig {
            path: "/etc/passwd".to_string(),
            data: passwd,
            symlink: false,
            directory: false,
            // Take defaults
            mode: None,
            uid: None,
            gid: None,
            recursive_chown: false,
        }.create(&output_dir)?;
    }

    if !shadow.is_empty() {
        FileConfig {
            path: "/etc/shadow".to_string(),
            data: shadow,
            symlink: false,
            directory: false,
            mode: Some(0o0600),
            uid: Some(0),
            gid: Some(0),
            recursive_chown: false,
        }.create(&output_dir)?;
    }

    Ok(())
}

pub fn with_redoxfs<D, T, F>(disk: D, password_opt: Option<&[u8]>, callback: F)
    -> Result<T> where
        D: Disk + Send + 'static,
        F: FnOnce(&Path) -> Result<T>
{
    let mount_path = if cfg!(target_os = "redox") {
        "file/redox_installer"
    } else {
        "/tmp/redox_installer"
    };

    if cfg!(not(target_os = "redox")) {
        if ! Path::new(mount_path).exists() {
            fs::create_dir(mount_path)?;
        }
    }

    let ctime = SystemTime::now().duration_since(UNIX_EPOCH)?;
    let fs = FileSystem::create(
        disk,
        password_opt,
        ctime.as_secs(),
        ctime.subsec_nanos()
    ).map_err(syscall_error)?;

    let (tx, rx) = channel();
    let join_handle = thread::spawn(move || {
        let res = redoxfs::mount(
            fs,
            mount_path,
            |real_path| {
                tx.send(Ok(real_path.to_owned())).unwrap();
            }
        );
        match res {
            Ok(()) => (),
            Err(err) => {
                tx.send(Err(err)).unwrap();
            },
        };
    });

    let res = match rx.recv() {
        Ok(ok) => match ok {
            Ok(real_path) => callback(&real_path),
            Err(err) => return Err(err.into()),
        },
        Err(_) => return Err(io::Error::new(
            io::ErrorKind::NotConnected,
            "redoxfs thread did not send a result"
        ).into()),
    };

    if cfg!(target_os = "redox") {
        fs::remove_file(format!(":{}", mount_path))?;
    } else {
        let status_res = if cfg!(target_os = "linux") {
            Command::new("fusermount")
                .arg("-u")
                .arg(mount_path)
                .status()
        } else {
            Command::new("umount")
                .arg(mount_path)
                .status()
        };

        let status = status_res?;
        if ! status.success() {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                "redoxfs umount failed"
            ).into());
        }
    }

    join_handle.join().unwrap();

    res
}

pub fn fetch_bootloaders<S: AsRef<str>>(cookbook: Option<S>, live: bool) -> Result<(Vec<u8>, Vec<u8>)> {
    //TODO: make it safe to run this concurrently
    let bootloader_dir = "/tmp/redox_installer_bootloader";
    if Path::new(bootloader_dir).exists() {
        fs::remove_dir_all(&bootloader_dir)?;
    }

    fs::create_dir(bootloader_dir)?;

    let mut bootloader_config = Config::default();
    bootloader_config.packages.insert("bootloader".to_string(), PackageConfig::default());
    install_packages(&bootloader_config, bootloader_dir, cookbook.as_ref());

    let boot_dir = Path::new(bootloader_dir).join("boot");
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
    Ok((
        if bios_path.exists() {
            fs::read(bios_path)?
        } else {
            Vec::new()
        },
        if efi_path.exists() {
            fs::read(efi_path)?
        } else {
            Vec::new()
        },
    ))
}

//TODO: make bootloaders use Option, dynamically create BIOS and EFI partitions
pub fn with_whole_disk<P, F, T>(disk_path: P, bootloader_bios: &[u8], bootloader_efi: &[u8], password_opt: Option<&[u8]>, callback: F)
    -> Result<T> where
        P: AsRef<Path>,
        F: FnOnce(&Path) -> Result<T>
{
    let target = get_target();

    let bootloader_efi_name = match target.as_str() {
        "aarch64-unknown-redox" => "BOOTAA64.EFI",
        "i686-unknown-redox" => "BOOTIA32.EFI",
        "x86_64-unknown-redox" => "BOOTX64.EFI",
        _ => {
            return Err(format_err!("target '{}' not supported", target));
        }
    };

    // Open disk and read metadata
    eprintln!("Opening disk {}", disk_path.as_ref().display());
    let mut disk_file = DiskWrapper::open(disk_path.as_ref())?;
    let disk_size = disk_file.size();
    let block_size = disk_file.block_size() as u64;
    let gpt_block_size = match block_size {
        512 => gpt::disk::LogicalBlockSize::Lb512,
        _ => {
            // TODO: support (and test) other block sizes
            return Err(format_err!("block size {} not supported", block_size));
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
    let efi_end = efi_start + (mibi / block_size) - 1;

    // The rest of the disk is RedoxFS, reserving the GPT table mirror at the end of disk
    let redoxfs_start = efi_end + 1;
    let redoxfs_end = ((((disk_size - gpt_reserved) / mibi) * mibi) / block_size) - 1;

    // Format and install BIOS partition
    {
        // Write BIOS bootloader to disk
        eprintln!("Write bootloader with size {:#x}", bootloader_bios.len());
        disk_file.seek(SeekFrom::Start(0))?;
        disk_file.write_all(&bootloader_bios)?;

        // Replace MBR tables with protective MBR
        let mbr_blocks = ((disk_size + block_size - 1) / block_size) - 1;
        eprintln!("Writing protective MBR with disk blocks {:#x}", mbr_blocks);
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
        partitions.insert(partition_id, gpt::partition::Partition {
            part_type_guid: gpt::partition_types::BIOS,
            part_guid: uuid::Uuid::new_v4(),
            first_lba: bios_start,
            last_lba: bios_end,
            flags: 0, // TODO
            name: "BIOS".to_string(),
        });
        partition_id += 1;

        // Add EFI boot partition
        partitions.insert(partition_id, gpt::partition::Partition {
            part_type_guid: gpt::partition_types::EFI,
            part_guid: uuid::Uuid::new_v4(),
            first_lba: efi_start,
            last_lba: efi_end,
            flags: 0, // TODO
            name: "EFI".to_string(),
        });
        partition_id += 1;

        // Add RedoxFS partition
        partitions.insert(partition_id, gpt::partition::Partition {
            //TODO: Use REDOX_REDOXFS type (needs GPT crate changes)
            part_type_guid: gpt::partition_types::LINUX_FS,
            part_guid: uuid::Uuid::new_v4(),
            first_lba: redoxfs_start,
            last_lba: redoxfs_end,
            flags: 0,
            name: "REDOX".to_string(),
        });

        eprintln!("Writing GPT tables: {:#?}", partitions);

        // Initialize GPT table
        gpt_disk.update_partitions(partitions)?;

        // Write partition layout, returning disk file
        gpt_disk.write()?;
    }

    // Format and install EFI partition
    {
        let disk_efi_start = efi_start * block_size;
        let disk_efi_end = (efi_end + 1) * block_size;
        let mut disk_efi = fscommon::StreamSlice::new(
            &mut disk_file,
            disk_efi_start,
            disk_efi_end,
        )?;

        eprintln!("Formatting EFI partition with size {:#x}", disk_efi_end - disk_efi_start);
        fatfs::format_volume(&mut disk_efi, fatfs::FormatVolumeOptions::new())?;

        eprintln!("Opening EFI partition");
        let fs = fatfs::FileSystem::new(&mut disk_efi, fatfs::FsOptions::new())?;

        eprintln!("Creating EFI directory");
        let root_dir = fs.root_dir();
        root_dir.create_dir("EFI")?;

        eprintln!("Creating EFI/BOOT directory");
        let efi_dir = root_dir.open_dir("EFI")?;
        efi_dir.create_dir("BOOT")?;

        eprintln!("Writing EFI/BOOT/{} file with size {:#x}", bootloader_efi_name, bootloader_efi.len());
        let boot_dir = efi_dir.open_dir("BOOT")?;
        let mut file = boot_dir.create_file(bootloader_efi_name)?;
        file.truncate()?;
        file.write_all(&bootloader_efi)?;
    }

    // Format and install RedoxFS partition
    eprintln!("Installing to RedoxFS partition with size {:#x}", (redoxfs_end - redoxfs_start) * block_size);
    let disk_redoxfs = DiskIo(fscommon::StreamSlice::new(
        disk_file,
        redoxfs_start * block_size,
        (redoxfs_end + 1) * block_size
    )?);
    with_redoxfs(
        disk_redoxfs,
        password_opt,
        callback
    )
}

pub fn install<P, S>(config: Config, output: P, cookbook: Option<S>, live: bool)
    -> Result<()> where
        P: AsRef<Path>,
        S: AsRef<str>,
{
    println!("Install {:#?} to {}", config, output.as_ref().display());

    if output.as_ref().is_dir() {
        install_dir(config, output, cookbook)
    } else {
        let (bootloader_bios, bootloader_efi) = fetch_bootloaders(cookbook.as_ref(), live)?;
        with_whole_disk(output, &bootloader_bios, &bootloader_efi, None,
            move |mount_path| {
                install_dir(config, mount_path, cookbook)
            }
        )
    }
}
