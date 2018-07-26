#![deny(warnings)]

#[macro_use]
extern crate serde_derive;
extern crate argon2rs;
extern crate liner;
extern crate failure;
extern crate pkgutils;
extern crate rand;
extern crate termion;

mod config;

pub use config::Config;

use argon2rs::verifier::Encoded;
use argon2rs::{Argon2, Variant};
use failure::{Error, err_msg};
use rand::{OsRng, Rng};
use termion::input::TermRead;
use pkgutils::{Repo, Package};

use std::{env, fs};
use std::ffi::OsStr;
use std::io::{self, stderr, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::symlink;
use std::path::Path;
use std::process::{self, Command};
use std::str::FromStr;

type Result<T> = std::result::Result<T, Error>;

const REMOTE: &'static str = "https://static.redox-os.org/pkg";
const TARGET: &'static str = "x86_64-unknown-redox";
const SYSROOT: &'static str = "SYSROOT";

/// Converts a password to a serialized argon2rs hash, understandable
/// by redox_users. If the password is blank, the hash is blank.
fn hash_password(password: &str) -> Result<String> {
    if password != "" {
        let a2 = Argon2::new(10, 1, 4096, Variant::Argon2i)?;
        let salt = format!("{:X}", OsRng::new()?.next_u64());
        let enc = Encoded::new(
            a2,
            password.as_bytes(),
            salt.as_bytes(),
            &[],
            &[]
        );

        Ok(String::from_utf8(enc.to_u8())?)
    } else {
        Ok("".to_string())
    }
}

fn unwrap_or_prompt<T: FromStr>(option: Option<T>, context: &mut liner::Context, prompt: &str) -> Result<T> {
    match option {
        Some(t) => Ok(t),
        None => {
            let line = context.read_line(prompt, &mut |_| {})?;
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
    // TODO: Remove this debug msg
    println!("\nPass: {:?}; ConfPass: {:?};", password, confirm_password);

    // Note: Actually comparing two Option<String> values
    if confirm_password == password {
        Ok(password.unwrap_or("".to_string()))
    } else {
        Err(err_msg("passwords do not match"))
    }
}

fn install_packages<S: AsRef<str>>(config: &Config, dest: &str, cookbook: Option<S>) {
    let mut repo = Repo::new(TARGET);
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

        for (packagename, _package) in &config.packages {
            println!("Installing package {}", packagename);
            let path = format!("{}/{}/repo/{}/{}.tar.gz",
                               env::current_dir().unwrap().to_string_lossy(),
                               cookbook.as_ref(), TARGET, packagename);
            Package::from_path(&path).unwrap().install(dest).unwrap();
        }
    } else {
        for (packagename, _package) in &config.packages {
            println!("Installing package {}", packagename);
            repo.fetch(&packagename).unwrap().install(dest).unwrap();
        }
    }
}

pub fn install<P: AsRef<Path>, S: AsRef<str>>(config: Config, output_dir: P, cookbook: Option<S>) -> Result<()> {
    
    /// Creates a directory relative to the output directory
    fn create_dir_relative<P: AsRef<Path>>(path: P) -> Result<()> {
        let root = env::var(SYSROOT)?;
        let target_dir = Path::new(&root)
            .join(path.as_ref());
        println!("Create directory {}", target_dir.display());
        fs::create_dir_all(&target_dir)?;
        Ok(())
    }
    
    /// Creates a file relative to the output directory
    fn create_file_relative<P: AsRef<Path>>(path: P, data: &[u8], is_symlink: bool) -> Result<()> {
        let root = env::var(SYSROOT)?;
        let target_file = Path::new(&root)
            .join(path.as_ref());
        
        if let Some(parent) = target_file.parent() {
            println!("Create file parent {}", parent.display());
            fs::create_dir_all(parent)?;
        }
        
        if is_symlink {
            println!("Create symlink {}", target_file.display());
            symlink(&OsStr::from_bytes(data), &target_file)?;
        } else {
            println!("Create file {}", target_file.display());
            let mut file = fs::File::create(&target_file)?;
            file.write_all(data)?;
        }
        Ok(())
    }
    
    let mut context = liner::Context::new();
    
    macro_rules! prompt {
        ($dst:expr, $def:expr, $($arg:tt)*) => (if config.general.prompt {
            match unwrap_or_prompt($dst, &mut context, &format!($($arg)*)) {
                Ok(res) => if res.is_empty() {
                    Ok($def)
                } else {
                    Ok(res)
                },
                Err(err) => Err(err)
            }
        } else {
            Ok($dst.unwrap_or($def))
        })
    }

    // Using an env var here to communicate the root dir to the functions
    //   instead of passing it as a param
    env::set_var(SYSROOT, output_dir.as_ref().as_os_str());

    let output_dir = output_dir.as_ref();
    println!("Install {:#?} to {}", config, output_dir.display());

    // TODO: Mount disk if output is a file
    let output_dir = output_dir.to_owned();

    create_dir_relative("")?;

    install_packages(&config, output_dir.to_str().unwrap(), cookbook);

    for file in config.files {
        create_file_relative(file.path.trim_matches('/'), file.data.as_bytes(), file.symlink)?;
    }

    let mut passwd = String::new();
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

        create_dir_relative(home.trim_matches('/'))?;

        let password = hash_password(&password)?;

        passwd.push_str(&format!("{};{};{};{};{};file:{};file:{}\n", username, password, uid, gid, name, home, shell));
    }

    if !passwd.is_empty() {
        create_file_relative("etc/passwd", passwd.as_bytes(), false)?;
    }

    Ok(())
}
