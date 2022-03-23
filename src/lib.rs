#[macro_use]
extern crate serde_derive;
extern crate argon2;
extern crate libc;
extern crate liner;
extern crate failure;
extern crate pkgutils;
extern crate rand;
extern crate redoxfs;
extern crate syscall;
extern crate termion;

mod config;

pub use config::Config;
use config::file::FileConfig;
use config::package::PackageConfig;

use failure::{Error, err_msg};
use rand::{RngCore, rngs::OsRng};
use redoxfs::{DiskFile, FileSystem};
use termion::input::TermRead;
use pkgutils::{Repo, Package};

use std::{
    env,
    fs,
    io::{self, stderr, Write},
    path::Path,
    process::{self, Command},
    str::FromStr,
    sync::mpsc::channel,
    time::{SystemTime, UNIX_EPOCH},
    thread,
};

pub(crate) type Result<T> = std::result::Result<T, Error>;

const REMOTE: &'static str = "https://static.redox-os.org/pkg";

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

fn unwrap_or_prompt<T: FromStr>(option: Option<T>, context: &mut liner::Context, prompt: &str) -> Result<T> {
    match option {
        Some(t) => Ok(t),
        None => {
            let line = context.read_line(
                prompt,
                None,
                &mut liner::BasicCompleter::new(Vec::<String>::new())
            )?;
            T::from_str(&line).map_err(|_err| err_msg("failed to parse input"))
        }
    }
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
    let target = &env::var("TARGET").unwrap_or(
        option_env!("TARGET").map_or(
            "x86_64-unknown-redox".to_string(),
            |x| x.to_string()
        )
    );

    let mut repo = Repo::new(target);
    repo.add_remote(REMOTE);

    if let Some(cookbook) = cookbook {
        let status = Command::new("./repo.sh")
            .current_dir(cookbook.as_ref())
            .args(config.packages.keys())
            .spawn()
            .unwrap()
            .wait()
            .unwrap();

        if !status.success() {
            write!(stderr(), "./repo.sh failed.").unwrap();
            process::exit(1);
        }

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
            gid: Some(gid)
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
            gid: None
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
            gid: Some(0)
        }.create(&output_dir)?;
    }

    Ok(())
}

pub fn with_redoxfs<P, T, F>(disk_path: P, password_opt: Option<&[u8]>, bootloader: &[u8], callback: F)
    -> Result<T> where
        P: AsRef<Path>,
        F: FnOnce(&Path) -> Result<T>
{
    let mount_path = if cfg!(target_os = "redox") {
        "file/redox_installer"
    } else {
        "/tmp/redox_installer"
    };

    let disk = DiskFile::open(disk_path).map_err(syscall_error)?;

    if cfg!(not(target_os = "redox")) {
        if ! Path::new(mount_path).exists() {
            fs::create_dir(mount_path)?;
        }
    }

    let ctime = SystemTime::now().duration_since(UNIX_EPOCH)?;
    let fs = FileSystem::create_reserved(
        disk,
        password_opt,
        bootloader,
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

pub fn install<P, S>(config: Config, output: P, cookbook: Option<S>)
    -> Result<()> where
        P: AsRef<Path>,
        S: AsRef<str>,
{
    println!("Install {:#?} to {}", config, output.as_ref().display());

    if output.as_ref().is_dir() {
        install_dir(config, output, cookbook)
    } else {
        let bootloader = {
            //TODO: make it safe to
            let bootloader_dir = "/tmp/redox_installer_bootloader";
            if Path::new(bootloader_dir).exists() {
                fs::remove_dir_all(&bootloader_dir)?;
            }

            fs::create_dir(bootloader_dir)?;

            let mut bootloader_config = Config::default();
            bootloader_config.packages.insert("bootloader".to_string(), PackageConfig::default());
            install_packages(&bootloader_config, bootloader_dir, cookbook.as_ref());

            let mut bootloader = fs::read(Path::new(bootloader_dir).join("bootloader"))?;

            // Pad to 1 MiB
            while bootloader.len() < 1024 * 1024 {
                bootloader.push(0);
            }

            bootloader
        };

        with_redoxfs(output, None, &bootloader, move |mount_path| -> Result<()> {
            install_dir(config, mount_path, cookbook.as_ref())
        })
    }
}
