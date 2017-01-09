extern crate liner;
extern crate rand;
extern crate termion;
extern crate userutils;

use self::rand::Rng;
use self::termion::input::TermRead;

use std::io::{self, Write};
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
            Ok($def)
        })
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

        println!("Creating user {}:", username);
        println!("\tPassword: {}", password);
        println!("\tUID: {}", uid);
        println!("\tGID: {}", gid);
        println!("\tName: {}", name);
        println!("\tHome: {}", home);
        println!("\tShell: {}", shell);

        passwd.push_str(&format!("{};{};{};{};{};{};{}\n", username, password, uid, gid, name, home, shell));
    }

    print!("/etc/passwd:\n{}", passwd);

    Ok(())
}
