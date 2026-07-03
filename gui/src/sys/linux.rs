use std::{fs, os::unix::process::CommandExt, process::Command};

pub fn ask_root() -> Result<(), String> {
    let Ok(exe_path) = std::env::current_exe() else {
        return Err(format!("Could not determine current_exe"));
    };
    let mut cmd = Command::new("pkexec");
    cmd.arg("env");
    for (key, value) in std::env::vars() {
        if [
            "DISPLAY",
            "WAYLAND_DISPLAY",
            "XAUTHORITY",
            "XDG_RUNTIME_DIR",
            "DBUS_SESSION_BUS_ADDRESS",
        ]
        .contains(&key.as_str())
        {
            cmd.arg(format!("{}={}", key, value));
        }
    }
    cmd.arg(exe_path);
    // will never return unless fail
    let e = cmd.exec();
    return Err(format!("Failed to escalate: {e}"));
}

pub fn is_root() -> bool {
    let euid = unsafe { libc::geteuid() };
    euid == 0
}

pub fn disk_paths() -> Result<Vec<(String, bool, u64)>, String> {
    let mut paths = Vec::new();

    let entries = match fs::read_dir("/sys/class/block") {
        Ok(entries) => entries,
        Err(err) => return Err(format!("failed to read /sys/class/block: {}", err)),
    };

    for entry_res in entries {
        if let Ok(entry) = entry_res {
            let file_name = entry.file_name();
            let name = file_name.to_string_lossy();

            if name.starts_with("loop")
                || name.starts_with("ram")
                || name.starts_with("sr")
                || name.starts_with("zram")
            {
                continue;
            }

            let path = entry.path();
            let is_partition = path.join("partition").exists();
            let size_path = path.join("size");
            let Ok(size_str) = fs::read_to_string(&size_path) else {
                continue;
            };
            let Ok(sectors) = size_str.trim().parse::<u64>() else {
                continue;
            };
            // /sys/class/block/*/size is 512 (cmiiw)
            let size_bytes = sectors * 512;
            if size_bytes == 0 {
                continue;
            }
            paths.push((format!("/dev/{}", name), is_partition, size_bytes));
            continue;
        }
    }

    paths.sort_by(|a, b| a.0.cmp(&b.0));

    Ok(paths)
}
