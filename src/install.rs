extern crate liner;
extern crate pkgutils;
extern crate rand;
extern crate termion;
extern crate userutils;

use self::rand::Rng;
use self::termion::input::TermRead;
use self::pkgutils::{Repo, Package};

use std::{env, fs};
use std::io::{self, stderr, Write};
use std::str::FromStr;
use std::process::{self, Command};

use config::Config;

const REMOTE: &'static str = "https://static.redox-os.org/pkg";
const TARGET: &'static str = "x86_64-unknown-redox";

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

fn install_packages(config: &Config, dest: &str, cookbook: Option<&str>) {
    let mut repo = Repo::new(TARGET);
    repo.add_remote(REMOTE);

    if let Some(cookbook) = cookbook {
        let status = Command::new("./update-packages.sh")
            .current_dir(cookbook)
            .args(config.packages.keys())
            .spawn()
            .unwrap()
            .wait()
            .unwrap();

        if !status.success() {
            write!(stderr(), "./update-package.sh failed.").unwrap();
            process::exit(1);
        }

        for (packagename, _package) in &config.packages {
            println!("Installing package {}", packagename);
            let path = format!("{}/{}/repo/{}/{}.tar",
                               env::current_dir().unwrap().to_string_lossy(),
                               cookbook, TARGET, packagename);
            Package::from_path(&path).unwrap().install(dest).unwrap();
        }
    } else {
        for (packagename, _package) in &config.packages {
            println!("Installing package {}", packagename);
            repo.fetch(&packagename).unwrap().install(dest).unwrap();
        }
    }
}

pub fn install(config: Config, cookbook: Option<&str>) -> Result<(), String> {
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
        let path = prompt!(config.general.sysroot.clone(), "sysroot".to_string(), "sysroot [sysroot]: ")?;
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
            if let Some(parent) = path.parent() {
                println!("Create file parent {}", parent.display());
                fs::create_dir_all(parent).map_err(|err| format!("failed to create file parent {}: {}", parent.display(), err))?;
            }
            println!("Create file {}", path.display());
            let mut file = fs::File::create(&path).map_err(|err| format!("failed to create {}: {}", path.display(), err))?;
            file.write_all($data).map_err(|err| format!("failed to write {}: {}", path.display(), err))?;
        }};
    }

    dir!("");

    install_packages(&config, sysroot.to_str().unwrap(), cookbook);

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
    if ! passwd.is_empty() {
        file!("etc/passwd", passwd.as_bytes());
    }

    Ok(())
}
