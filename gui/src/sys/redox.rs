use std::fs;

pub fn ask_root(password: &str) -> Result<(), String> {
    let file = libredox::call::open("/scheme/sudo", libredox::flag::O_CLOEXEC, 0)
        .map_err(|err| err.to_string())?;

    libredox::call::write(file, password.as_bytes()).map_err(|err| err.to_string())?;

    // FIXME move to libredox
    unsafe extern "C" {
        safe fn redox_cur_procfd_v0() -> usize;
    }

    // Elevate privileges of our own process with help from the sudo daemon
    syscall::sendfd(
        file,
        syscall::dup(redox_cur_procfd_v0(), &[]).map_err(|err| err.to_string())?,
        0,
        0,
    )
    .map_err(|err| err.to_string())?;

    Ok(())
}

pub fn is_root() -> bool {
    let euid = libredox::call::geteuid().unwrap();
    euid == 0
}

pub fn disk_paths() -> Result<Vec<(String, bool, u64)>, String> {
    let mut schemes = Vec::new();
    match fs::read_dir("/scheme/") {
        Ok(entries) => {
            for entry_res in entries {
                if let Ok(entry) = entry_res {
                    let path = entry.path();
                    if let Ok(path_str) = path.into_os_string().into_string() {
                        let scheme = path_str.trim_start_matches("/scheme/").trim_matches('/');
                        if scheme.starts_with("disk") {
                            if scheme == "disk/live" {
                                // Skip live disks
                                continue;
                            }

                            schemes.push(format!("/scheme/{}", scheme));
                        }
                    }
                }
            }
        }
        Err(err) => {
            return Err(format!("failed to list schemes: {}", err));
        }
    }

    let mut paths = Vec::new();
    for scheme in schemes {
        let is_dir = fs::metadata(&scheme).map(|x| x.is_dir()).unwrap_or(false);
        if !is_dir {
            continue;
        }
        match fs::read_dir(&scheme) {
            Ok(entries) => {
                for entry_res in entries {
                    let Ok(entry) = entry_res else {
                        continue;
                    };
                    let Ok(file_name) = entry.file_name().into_string() else {
                        continue;
                    };
                    let Ok(path) = entry.path().into_os_string().into_string() else {
                        continue;
                    };
                    let Ok(metadata) = entry.metadata() else {
                        continue;
                    };
                    let is_partition = file_name.contains(p);
                    let size = metadata.len();
                    if size == 0 {
                        continue;
                    }
                    paths.push((path, is_partition, size));
                }
            }
            Err(err) => {
                return Err(format!("failed to list {:?}: {}", scheme, err));
            }
        }
    }

    Ok(paths)
}
