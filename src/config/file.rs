
use Result;
use libc::{gid_t, uid_t};

use std::io::{Error, Write};
use std::ffi::{CString, OsStr};
use std::fs::{self, File};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;

//type Result<T> = std::result::Result<T, Error>;

fn chown<P: AsRef<Path>>(path: P, uid: uid_t, gid: gid_t, recursive: bool) -> Result<()> {
    let path = path.as_ref();

    let c_path = CString::new(path.as_os_str().as_bytes()).unwrap();
    if unsafe { libc::chown(c_path.as_ptr(), uid, gid) } != 0 {
        return Err(Error::last_os_error().into());
    }

    if recursive && path.is_dir() {
        for entry_res in fs::read_dir(path)? {
            let entry = entry_res?;
            chown(entry.path(), uid, gid, recursive)?;
        }
    }

    Ok(())
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct FileConfig {
    pub path: String,
    pub data: String,
    #[serde(default)]
    pub symlink: bool,
    #[serde(default)]
    pub directory: bool,
    pub mode: Option<u32>,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    #[serde(default)]
    pub recursive_chown: bool,
}

// TODO: Rewrite impls
impl FileConfig {
    pub(crate) fn create<P: AsRef<Path>>(&self, prefix: P) -> Result<()> {
        let path = self.path.trim_start_matches('/');
        let target_file = prefix.as_ref()
            .join(path);

        if self.directory {
            println!("Create directory {}", target_file.display());
            fs::create_dir_all(&target_file)?;
            self.apply_perms(&target_file)?;
            return Ok(());
        } else if let Some(parent) = target_file.parent() {
            println!("Create file parent {}", parent.display());
            fs::create_dir_all(parent)?;
        }

        if self.symlink {
            println!("Create symlink {}", target_file.display());
            symlink(&OsStr::new(&self.data), &target_file)?;
            Ok(())
        } else {
            println!("Create file {}", target_file.display());
            let mut file = File::create(&target_file)?;
            file.write_all(self.data.as_bytes())?;

            self.apply_perms(target_file)
        }
    }

    fn apply_perms<P: AsRef<Path>>(&self, target: P) -> Result<()> {
        let path = target.as_ref();
        let mode = self.mode.unwrap_or_else(|| if self.directory {
                0o0755
            } else {
                0o0644
            });
        let uid = self.uid.unwrap_or(!0);
        let gid = self.gid.unwrap_or(!0);

        // chmod
        fs::set_permissions(path, fs::Permissions::from_mode(mode))?;

        // chown
        chown(path, uid, gid, self.recursive_chown)
    }
}
