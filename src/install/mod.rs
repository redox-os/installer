extern crate liner;
extern crate pkgutils;
extern crate rand;
extern crate tar;
extern crate termion;
extern crate userutils;

use self::rand::Rng;
use self::tar::{Archive, EntryType};
use self::termion::input::TermRead;

use std::{env, fs};
use std::io::{self, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::str::FromStr;

use config::Config;

fn unwrap_or_prompt<T: FromStr>(option: Option<T>, context: &mut liner::Context, prompt: &str) -> Result<T, String> {
    match option {
        None => {
            let line = context.read_line(prompt, &mut |_| {}).map_err(|err| format!("failed to read line: {}", err))?;
            T::from_str(&line).map_err(|_| format!("failed to parse '{}'", line))
        },
        Some(t) => Ok(t)
    }
}

fn prompt_password(prompt: &str, confirm_prompt: &str) -> Result<String, String> {
    let stdin = io::stdin();
    let mut stdin = stdin.lock();
    let stdout = io::stdout();
    let mut stdout = stdout.lock();

    stdout.write(prompt.as_bytes()).map_err(|err| format!("failed to write to stdout: {}", err))?;
    stdout.flush().map_err(|err| format!("failed to flush stdout: {}", err))?;
    if let Some(password) = stdin.read_passwd(&mut stdout).map_err(|err| format!("failed to read password: {}", err))? {
        stdout.write(b"\n").map_err(|err| format!("failed to write to stdout: {}", err))?;
        stdout.flush().map_err(|err| format!("failed to flush stdout: {}", err))?;

        if password.is_empty() {
            Ok(password)
        } else {
            stdout.write(confirm_prompt.as_bytes()).map_err(|err| format!("failed to write to stdout: {}", err))?;
            stdout.flush().map_err(|err| format!("failed to flush stdout: {}", err))?;
            if let Some(confirm_password) = stdin.read_passwd(&mut stdout).map_err(|err| format!("failed to read password: {}", err))? {
                stdout.write(b"\n").map_err(|err| format!("failed to write to stdout: {}", err))?;
                stdout.flush().map_err(|err| format!("failed to flush stdout: {}", err))?;

                if confirm_password == password {
                    let salt = format!("{:X}", rand::OsRng::new().unwrap().next_u64());
                    Ok(userutils::Passwd::encode(&password, &salt))
                } else {
                    Err("passwords do not match".to_string())
                }
            } else {
                Err("passwords do not match".to_string())
            }
        }
    } else {
        Ok(String::new())
    }
}

fn extract_inner<T: Read>(ar: &mut Archive<T>, root: &Path) -> io::Result<()> {
    for entry_result in try!(ar.entries()) {
        let mut entry = try!(entry_result);
        match entry.header().entry_type() {
            EntryType::Regular => {
                let mut file = {
                    let mut path = root.to_path_buf();
                    path.push(try!(entry.path()));
                    println!("Extract file {}", path.display());
                    if let Some(parent) = path.parent() {
                        try!(fs::create_dir_all(parent));
                    }
                    try!(
                        fs::OpenOptions::new()
                            .read(true)
                            .write(true)
                            .truncate(true)
                            .create(true)
                            .mode(entry.header().mode().unwrap_or(644))
                            .open(path)
                    )
                };
                try!(io::copy(&mut entry, &mut file));
            },
            EntryType::Directory => {
                let mut path = root.to_path_buf();
                path.push(try!(entry.path()));
                println!("Extract directory {}", path.display());
                try!(fs::create_dir_all(path));
            },
            other => {
                panic!("Unsupported entry type {:?}", other);
            }
        }
    }

    Ok(())
}

pub fn install(config: Config) -> Result<(), String> {
    println!("Install {:#?}", config);

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

    let sysroot = {
        let mut wd = env::current_dir().map_err(|err| format!("failed to get current dir: {}", err))?;
        let path = prompt!(config.general.sysroot, "sysroot".to_string(), "sysroot [sysroot]: ")?;
        wd.push(path);
        wd
    };

    macro_rules! dir {
        ($path:expr) => {{
            let mut path = sysroot.clone();
            path.push($path);
            println!("Create directory {}", path.display());
            fs::create_dir_all(&path).map_err(|err| format!("failed to create {}: {}", path.display(), err))?;
        }};
    }

    macro_rules! file {
        ($path:expr, $data:expr) => {{
            let mut path = sysroot.clone();
            path.push($path);
            println!("Create file {}", path.display());
            let mut file = fs::File::create(&path).map_err(|err| format!("failed to create {}: {}", path.display(), err))?;
            file.write_all($data).map_err(|err| format!("failed to write {}: {}", path.display(), err))?;
        }};
    }

    dir!("");

    for (packagename, _package) in config.packages {
        let remote_path = format!("{}/{}.tar", pkgutils::REPO_REMOTE, $name);
        let local_path = format!("pkg/{}.tar", $name);
        if let Some(parent) = Path::new(&local_path).parent() {
            println!("Create package repository {}", parent.display());
            fs::create_dir_all(parent).map_err(|err| format!("failed to create package repository {}: {}", parent.display(), err))?;
        }
        println!("Download package {} to {}", remote_path, local_path);
        pkgutils::download(&remote_path, &local_path).map_err(|err| format!("failed to download {} to {}: {}", remote_path, local_path, err))?;

        let path = Path::new(&local_path);
        println!("Extract package {}", path.display());
        let file = fs::File::open(&path).map_err(|err| format!("failed to open {}: {}", path.display(), err))?;
        extract_inner(&mut Archive::new(file), &sysroot).map_err(|err| format!("failed to extract {}: {}", path.display(), err))?;
    }

    for file in config.files {
        file!(file.path.trim_matches('/'), file.data.as_bytes());
    }

    let mut passwd = String::new();
    let mut next_uid = 1000;
    for (username, user) in config.users {
        let password = if let Some(password) = user.password {
            password
        } else if config.general.prompt {
            prompt_password(&format!("{}: enter password: ", username), &format!("{}: confirm password: ", username))?
        } else {
            String::new()
        };

        let uid = user.uid.unwrap_or(next_uid);

        if uid >= next_uid {
            next_uid = uid + 1;
        }

        let gid = user.gid.unwrap_or(uid);

        let name = prompt!(user.name, username.clone(), "{}: name [{}]: ", username, username)?;
        let home = prompt!(user.home, format!("/home/{}", username), "{}: home [/home/{}]: ", username, username)?;
        let shell = prompt!(user.shell, "/bin/ion".to_string(), "{}: shell [/bin/ion]: ", username)?;

        println!("Adding user {}:", username);
        println!("\tPassword: {}", password);
        println!("\tUID: {}", uid);
        println!("\tGID: {}", gid);
        println!("\tName: {}", name);
        println!("\tHome: {}", home);
        println!("\tShell: {}", shell);

        dir!(home.trim_matches('/'));

        passwd.push_str(&format!("{};{};{};{};{};{};{}\n", username, password, uid, gid, name, home, shell));
    }
    file!("etc/passwd", passwd.as_bytes());

    Ok(())
}
