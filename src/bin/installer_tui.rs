use anyhow::{anyhow, bail, Result};
use pkgar::{ext::EntryExt, PackageHead};
use pkgar_core::PackageSrc;
use pkgar_keys::PublicKeyFile;
use redox_installer::{with_whole_disk, Config, DiskOption};
use std::{
    ffi::OsStr,
    fs,
    io::{self, Read, Write},
    os::unix::fs::{symlink, MetadataExt, OpenOptionsExt},
    path::Path,
    process,
};
use termion::input::TermRead;

#[cfg(not(target_os = "redox"))]
fn disk_paths(_paths: &mut Vec<(String, u64)>) {}

#[cfg(target_os = "redox")]
fn disk_paths(paths: &mut Vec<(String, u64)>) {
    let mut schemes = Vec::new();
    match fs::read_dir("/scheme") {
        Ok(entries) => {
            for entry_res in entries {
                if let Ok(entry) = entry_res {
                    let path = entry.path();
                    if let Ok(path_str) = path.into_os_string().into_string() {
                        let scheme = path_str.trim_start_matches(':').trim_matches('/');
                        if scheme.starts_with("disk") {
                            if scheme == "disk/live" {
                                // Skip live disks
                                continue;
                            }

                            schemes.push(format!("{}:", scheme));
                        }
                    }
                }
            }
        }
        Err(err) => {
            eprintln!("installer_tui: failed to list schemes: {}", err);
        }
    }

    for scheme in schemes {
        let is_dir = fs::metadata(&scheme).map(|x| x.is_dir()).unwrap_or(false);
        if is_dir {
            match fs::read_dir(&scheme) {
                Ok(entries) => {
                    for entry_res in entries {
                        if let Ok(entry) = entry_res {
                            if let Ok(file_name) = entry.file_name().into_string() {
                                if file_name.contains('p') {
                                    // Skip partitions
                                    continue;
                                }

                                if let Ok(path) = entry.path().into_os_string().into_string() {
                                    if let Ok(metadata) = entry.metadata() {
                                        let size = metadata.len();
                                        if size > 0 {
                                            paths.push((path, size));
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                Err(err) => {
                    eprintln!("installer_tui: failed to list '{}': {}", scheme, err);
                }
            }
        }
    }
}

const KIB: u64 = 1024;
const MIB: u64 = 1024 * KIB;
const GIB: u64 = 1024 * MIB;
const TIB: u64 = 1024 * GIB;

fn format_size(size: u64) -> String {
    if size >= 4 * TIB {
        format!("{:.1} TiB", size as f64 / TIB as f64)
    } else if size >= GIB {
        format!("{:.1} GiB", size as f64 / GIB as f64)
    } else if size >= MIB {
        format!("{:.1} MiB", size as f64 / MIB as f64)
    } else if size >= KIB {
        format!("{:.1} KiB", size as f64 / KIB as f64)
    } else {
        format!("{} B", size)
    }
}

fn copy_file(src: &Path, dest: &Path, buf: &mut [u8]) -> Result<()> {
    if let Some(parent) = dest.parent() {
        // Parent may be a symlink
        if !parent.is_symlink() {
            match fs::create_dir_all(&parent) {
                Ok(()) => (),
                Err(err) => {
                    bail!("failed to create directory {}: {}", parent.display(), err);
                }
            }
        }
    }

    let metadata = match fs::symlink_metadata(&src) {
        Ok(ok) => ok,
        Err(err) => {
            bail!("failed to read metadata of {}: {}", src.display(), err);
        }
    };

    if metadata.file_type().is_symlink() {
        let real_src = match fs::read_link(&src) {
            Ok(ok) => ok,
            Err(err) => {
                bail!("failed to read link {}: {}", src.display(), err);
            }
        };

        match symlink(&real_src, &dest) {
            Ok(()) => (),
            Err(err) => {
                bail!(
                    "failed to copy link {} ({}) to {}: {}",
                    src.display(),
                    real_src.display(),
                    dest.display(),
                    err
                );
            }
        }
    } else {
        let mut src_file = match fs::File::open(&src) {
            Ok(ok) => ok,
            Err(err) => {
                bail!("failed to open file {}: {}", src.display(), err);
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
                bail!("failed to create file {}: {}", dest.display(), err);
            }
        };

        loop {
            let count = match src_file.read(buf) {
                Ok(ok) => ok,
                Err(err) => {
                    bail!("failed to read file {}: {}", src.display(), err);
                }
            };

            if count == 0 {
                break;
            }

            match dest_file.write_all(&buf[..count]) {
                Ok(()) => (),
                Err(err) => {
                    bail!("failed to write file {}: {}", dest.display(), err);
                }
            }
        }
    }

    Ok(())
}

fn package_files(
    root_path: &Path,
    config: &mut Config,
    files: &mut Vec<String>,
) -> Result<(), pkgar::Error> {
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

fn choose_disk() -> String {
    let mut paths = Vec::new();
    disk_paths(&mut paths);
    loop {
        for (i, (path, size)) in paths.iter().enumerate() {
            eprintln!("\x1B[1m{}\x1B[0m: {}: {}", i + 1, path, format_size(*size));
        }

        if paths.is_empty() {
            eprintln!("installer_tui: no drives found");
            process::exit(1);
        } else {
            eprint!("Select a drive from 1 to {}: ", paths.len());

            let mut line = String::new();
            match io::stdin().read_line(&mut line) {
                Ok(0) => {
                    eprintln!("installer_tui: failed to read line: end of input");
                    process::exit(1);
                }
                Ok(_) => (),
                Err(err) => {
                    eprintln!("installer_tui: failed to read line: {}", err);
                    process::exit(1);
                }
            }

            match line.trim().parse::<usize>() {
                Ok(i) => {
                    if i >= 1 && i <= paths.len() {
                        break paths[i - 1].0.clone();
                    } else {
                        eprintln!("{} not from 1 to {}", i, paths.len());
                    }
                }
                Err(err) => {
                    eprintln!("invalid input: {}", err);
                }
            }
        }
    }
}

fn choose_password() -> Option<String> {
    eprint!("installer_tui: redoxfs password (empty for none): ");

    let password = io::stdin()
        .read_passwd(&mut io::stderr())
        .unwrap()
        .unwrap_or(String::new());

    eprintln!();

    if password.is_empty() {
        return None;
    }

    Some(password)
}

fn main() {
    let root_path = Path::new("/");

    let disk_path = choose_disk();

    let password_opt = choose_password();

    let bootloader_bios = {
        let path = root_path.join("boot").join("bootloader.bios");
        if path.exists() {
            match fs::read(&path) {
                Ok(ok) => ok,
                Err(err) => {
                    eprintln!("installer_tui: {}: failed to read: {}", path.display(), err);
                    process::exit(1);
                }
            }
        } else {
            Vec::new()
        }
    };

    let bootloader_efi = {
        let path = root_path.join("boot").join("bootloader.efi");
        if path.exists() {
            match fs::read(&path) {
                Ok(ok) => ok,
                Err(err) => {
                    eprintln!("installer_tui: {}: failed to read: {}", path.display(), err);
                    process::exit(1);
                }
            }
        } else {
            Vec::new()
        }
    };

    let disk_option = DiskOption {
        bootloader_bios: &bootloader_bios,
        bootloader_efi: &bootloader_efi,
        password_opt: password_opt.as_ref().map(|x| x.as_bytes()),
        efi_partition_size: None,
    };
    let res = with_whole_disk(&disk_path, &disk_option, |mount_path| -> Result<()> {
        let mut config: Config = Config::from_file(&root_path.join("filesystem.toml"))?;

        // Copy filesystem.toml, which is not packaged
        let mut files = vec!["filesystem.toml".to_string()];

        // Copy files from locally installed packages
        package_files(&root_path, &mut config, &mut files)
            // TODO: implement Error trait
            .map_err(|err| anyhow!("failed to read package files: {err}"))?;

        // Perform config install (after packages have been converted to files)
        eprintln!("configuring system");
        let cookbook: Option<&'static str> = None;
        redox_installer::install_dir(config, mount_path, cookbook)
            .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;

        // Sort and remove duplicates
        files.sort();
        files.dedup();

        // Install files
        let mut buf = vec![0; 4 * MIB as usize];
        for (i, name) in files.iter().enumerate() {
            eprintln!("copy {} [{}/{}]", name, i, files.len());

            let src = root_path.join(name);
            let dest = mount_path.join(name);
            copy_file(&src, &dest, &mut buf)?;
        }

        eprintln!("finished installing, unmounting filesystem");

        Ok(())
    });

    match res {
        Ok(()) => {
            eprintln!("installer_tui: installed successfully");
            process::exit(0);
        }
        Err(err) => {
            eprintln!("installer_tui: failed to install: {}", err);
            process::exit(1);
        }
    }
}
