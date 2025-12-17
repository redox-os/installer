use anyhow::Result;
use libc::{gid_t, uid_t};

use std::ffi::{CString, OsStr};
use std::fs::{self, File};
use std::io::{Error, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::Path;

#[cfg(feature = "installer")]
use redoxfs::{Disk, Node, Transaction, TreePtr};
#[cfg(feature = "installer")]
use crate::redoxfs_ops;

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

// TODO: Rewrite impls
impl crate::FileConfig {
    pub(crate) fn create<P: AsRef<Path>>(&self, prefix: P) -> Result<()> {
        let path = self.path.trim_start_matches('/');
        let target_file = prefix.as_ref().join(path);

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
        let mode = self
            .mode
            .unwrap_or_else(|| if self.directory { 0o0755 } else { 0o0644 });
        let uid = self.uid.unwrap_or(!0);
        let gid = self.gid.unwrap_or(!0);

        // chmod
        fs::set_permissions(path, fs::Permissions::from_mode(mode))?;

        // chown
        chown(path, uid, gid, self.recursive_chown)
    }

    /// Create file/directory/symlink using RedoxFS Transaction API
    #[cfg(feature = "installer")]
    pub fn create_in_tx<D: Disk>(
        &self,
        tx: &mut Transaction<D>,
        ctime: u64,
        ctime_nsec: u32,
    ) -> Result<TreePtr<Node>> {
        let path = Path::new(self.path.trim_start_matches('/'));
        let mode = self
            .mode
            .unwrap_or_else(|| if self.directory { 0o0755 } else { 0o0644 }) as u16;
        let uid = self.uid.unwrap_or(0);
        let gid = self.gid.unwrap_or(0);

        println!(
            "Create {} {} (mode={:o}, uid={}, gid={})",
            if self.directory {
                "directory"
            } else if self.symlink {
                "symlink"
            } else {
                "file"
            },
            path.display(),
            mode,
            uid,
            gid
        );

        redoxfs_ops::create_at_path(
            tx,
            path,
            self.directory,
            self.symlink,
            self.data.as_bytes(),
            mode,
            uid,
            gid,
            ctime,
            ctime_nsec,
        )
    }
}
