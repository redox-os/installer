use failure::format_err;
use iced::executor;
use iced::theme;
use iced::widget::{button, column, horizontal_space, progress_bar, radio, row, text, vertical_space};
use iced::{Alignment, Application, Command, Element, Length, Settings, Subscription, Theme};
use pkgar::{PackageHead, ext::EntryExt};
use pkgar_core::PackageSrc;
use pkgar_keys::PublicKeyFile;
use redox_installer::{Config, with_whole_disk};
use std::{
    ffi::OsStr,
    fs,
    io::{self, Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, symlink},
    path::Path,
};

fn main() -> iced::Result {
    let mut settings = Settings::default();
    settings.window.size = (608, 416);
    Window::run(settings)
}

fn disk_paths() -> Result<Vec<(String, u64)>, String> {
    let mut schemes = Vec::new();
    match fs::read_dir(":") {
        Ok(entries) => for entry_res in entries {
            if let Ok(entry) = entry_res {
                let path = entry.path();
                if let Ok(path_str) = path.into_os_string().into_string() {
                    let scheme = path_str.trim_start_matches(':').trim_matches('/');
                    if scheme.starts_with("disk") {
                        schemes.push(format!("{}:", scheme));
                    }
                }
            }
        },
        Err(err) => {
            return Err(format!("failed to list schemes: {}", err));
        }
    }

    let mut paths = Vec::new();
    for scheme in schemes {
        let is_dir = fs::metadata(&scheme)
            .map(|x| x.is_dir())
            .unwrap_or(false);
        if is_dir {
            match fs::read_dir(&scheme) {
                Ok(entries) => for entry_res in entries {
                    if let Ok(entry) = entry_res {
                        if let Ok(path) = entry.path().into_os_string().into_string() {
                            if let Ok(metadata) = entry.metadata() {
                                let size = metadata.len();
                                if size > 0 {
                                    paths.push((path, size));
                                }
                            }
                        }
                    }
                },
                Err(err) => {
                    return Err(format!("failed to list '{}': {}", scheme, err));
                }
            }
        }
    }

    Ok(paths)
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

fn copy_file(src: &Path, dest: &Path, buf: &mut [u8]) -> Result<(), failure::Error> {
    if let Some(parent) = dest.parent() {
        match fs::create_dir_all(&parent) {
            Ok(()) => (),
            Err(err) => {
                return Err(format_err!("failed to create directory {}: {}", parent.display(), err));
            }
        }
    }

    let metadata = match fs::symlink_metadata(&src) {
        Ok(ok) => ok,
        Err(err) => {
            return Err(format_err!("failed to read metadata of {}: {}", src.display(), err));
        },
    };

    if metadata.file_type().is_symlink() {
        let real_src = match fs::read_link(&src) {
            Ok(ok) => ok,
            Err(err) => {
                return Err(format_err!("failed to read link {}: {}", src.display(), err));
            }
        };

        match symlink(&real_src, &dest) {
            Ok(()) => (),
            Err(err) => {
                return Err(format_err!("failed to copy link {} ({}) to {}: {}", src.display(), real_src.display(), dest.display(), err));
            },
        }
    } else {
        let mut src_file = match fs::File::open(&src) {
            Ok(ok) => ok,
            Err(err) => {
                return Err(format_err!("failed to open file {}: {}", src.display(), err));
            }
        };

        let mut dest_file = match fs::OpenOptions::new().write(true).create_new(true).mode(metadata.mode()).open(&dest) {
            Ok(ok) => ok,
            Err(err) => {
                return Err(format_err!("failed to create file {}: {}", dest.display(), err));
            }
        };

        loop {
            let count = match src_file.read(buf) {
                Ok(ok) => ok,
                Err(err) => {
                    return Err(format_err!("failed to read file {}: {}", src.display(), err));
                }
            };

            if count == 0 {
                break;
            }

            match dest_file.write_all(&buf[..count]) {
                Ok(()) => (),
                Err(err) => {
                    return Err(format_err!("failed to write file {}: {}", dest.display(), err));
                }
            }
        }
    }

    Ok(())
}

fn package_files(root_path: &Path, config: &mut Config, files: &mut Vec<String>) -> Result<(), pkgar::Error> {
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
                files.push(
                    entry
                        .check_path()?
                        .to_str().unwrap()
                        .to_string()
                );
            }
            files.push(
                pkg_path
                    .strip_prefix(root_path).unwrap()
                    .to_str().unwrap()
                    .to_string()
            );
        }
    }

    Ok(())
}

fn install<F: FnMut(Message)>(disk_path: String, password_opt: Option<String>, mut f: F) {
    let mut progress = 0;

    macro_rules! message {
        ($($arg:tt)*) => {{
            f(Message::Install(
                progress,
                format!($($arg)*)
            ));
        }}
    }

    let root_path = Path::new("file:");

    message!("loading bootloader");
    let bootloader_bios = {
        let path = root_path.join("boot").join("bootloader.bios");
        if path.exists() {
            match fs::read(&path) {
                Ok(ok) => ok,
                Err(err) => {
                    f(Message::Error(
                        format!("{}: failed to read: {}", path.display(), err)
                    ));
                    return;
                }
            }
        } else {
            Vec::new()
        }
    };

    message!("loading bootloader.efi");
    let bootloader_efi = {
        let path = root_path.join("boot").join("bootloader.efi");
        if path.exists() {
            match fs::read(&path) {
                Ok(ok) => ok,
                Err(err) => {
                    f(Message::Error(
                        format!("{}: failed to read: {}", path.display(), err)
                    ));
                    return;
                }
            }
        } else {
            Vec::new()
        }
    };

    message!("formatting disk");
    let res = with_whole_disk(&disk_path, &bootloader_bios, &bootloader_efi, password_opt.as_ref().map(|x| x.as_bytes()), |mount_path| -> Result<(), failure::Error> {
        message!("loading filesystem.toml");
        let mut config: Config = {
            let path = root_path.join("filesystem.toml");
            match fs::read_to_string(&path) {
                Ok(config_data) => {
                    match toml::from_str(&config_data) {
                        Ok(config) => {
                            config
                        },
                        Err(err) => {
                            return Err(format_err!("{}: failed to decode: {}", path.display(), err));
                        }
                    }
                },
                Err(err) => {
                    return Err(format_err!("{}: failed to read: {}", path.display(), err));
                }
            }
        };

        // Copy filesystem.toml, which is not packaged
        let mut files = vec![
            "filesystem.toml".to_string(),
        ];

        // Copy files from locally installed packages
        message!("loading package files");
        if let Err(err) = package_files(&root_path, &mut config, &mut files) {
            return Err(format_err!("failed to read package files: {}", err));
        }

        // Sort and remove duplicates
        files.sort();
        files.dedup();

        let mut buf = vec![0; 4 * MIB as usize];
        for (i, name) in files.iter().enumerate() {
            progress = (i * 100) / files.len();
            message!("copy {} [{}/{}]", name, i, files.len());

            let src = root_path.join(name);
            let dest = mount_path.join(name);
            copy_file(&src, &dest, &mut buf)?;
        }

        message!("configuring system");
        let cookbook: Option<&'static str> = None;
        redox_installer::install_dir(config, mount_path, cookbook).map_err(|err| {
            io::Error::new(
                io::ErrorKind::Other,
                err
            )
        })?;

        progress = 100;
        message!("finished installing, unmounting filesystem");
        Ok(())
    });

    match res {
        Ok(()) => {
            message!("finished installing, ready to reboot");
            //TODO: reboot button?
        },
        Err(err) => {
            f(Message::Error(
                format!("failed to install: {}", err)
            ));
        }
    }
}

enum Page {
    Disk(Option<usize>),
    Install(usize, String),
    Error(String),
}

struct Window {
    page: Page,
    disk_paths: Vec<(String, u64)>,
    command_sender_opt: Option<std::sync::mpsc::Sender<(String, Option<String>)>>,
}

#[derive(Clone, Debug)]
enum Message {
    CommandSender(std::sync::mpsc::Sender<(String, Option<String>)>),
    DiskChoose(usize),
    DiskConfirm(usize),
    Install(usize, String),
    Error(String),
}

impl Application for Window {
    type Executor = executor::Default;
    type Flags = ();
    type Message = Message;
    type Theme = Theme;

    fn new(_flags: ()) -> (Self, Command<Message>) {
        //TODO: load in background
        let (page, disk_paths) = match disk_paths() {
            Ok(disk_paths) => (Page::Disk(None), disk_paths),
            Err(err) => (Page::Error(err), Vec::new()),
        };

        (
            Self {
                page,
                disk_paths,
                command_sender_opt: None,
            },
            Command::none(),
        )
    }

    fn title(&self) -> String {
        String::from("Redox OS Installer")
    }

    fn update(&mut self, message: Message) -> Command<Message> {
        match message {
            Message::CommandSender(command_sender) => {
                self.command_sender_opt = Some(command_sender);
            },
            Message::DiskChoose(disk_i) => {
                self.page = Page::Disk(Some(disk_i));
            },
            Message::DiskConfirm(disk_i) => {
                match self.disk_paths.get(disk_i) {
                    Some((disk_path, disk_size)) => match &self.command_sender_opt {
                        Some(command_sender) => {
                            match command_sender.send((disk_path.clone(), None)) {
                                Ok(()) => self.page = Page::Install(0, format!("Starting install...")),
                                Err(err) => {
                                    self.page = Page::Error(format!("failed to send command: {}", err));
                                },
                            }
                        },
                        None => {
                            self.page = Page::Error(format!("command sender not found"));
                        }
                    },
                    None => {
                        self.page = Page::Error(format!("invalid disk number {} chosen", disk_i));
                    },
                }
            },
            Message::Install(progress, description) => {
                self.page = Page::Install(progress, description);
            },
            Message::Error(err) => {
                self.page = Page::Error(err);
            },
        }
        Command::none()
    }

    fn view(&self) -> Element<Message> {
        let mut widgets = Vec::new();
        match &self.page {
            Page::Disk(disk_i_opt) => if ! self.disk_paths.is_empty() {
                widgets.push(text("Choose a drive:").size(24).into());

                for (disk_i, (disk_path, disk_size)) in self.disk_paths.iter().enumerate() {
                    widgets.push(
                        row![
                            radio(disk_path, disk_i, *disk_i_opt, Message::DiskChoose),
                            horizontal_space(Length::Fill),
                            text(format_size(*disk_size)),
                        ]
                        .into()
                    );
                }

                widgets.push(vertical_space(Length::Fill).into());

                let mut confirm_button = button("Confirm").style(theme::Button::Destructive);
                if let Some(disk_i) = *disk_i_opt {
                    confirm_button = confirm_button.on_press(Message::DiskConfirm(disk_i));
                }

                widgets.push(row![
                    horizontal_space(Length::Fill),
                    confirm_button,
                ].into());
            } else {
                widgets.push(text("Error: no drives found").into());
            },
            Page::Install(progress, description) => {
                widgets.push(text("Installation progress:").size(24).into());
                widgets.push(progress_bar(0.0..=100.0, *progress as f32).into());
                widgets.push(text(description).into());
            },
            Page::Error(err) => {
                widgets.push(text(format!("Error: {}", err)).into());
            }
        };

        column(widgets)
        .spacing(8)
        .padding(24)
        .align_items(Alignment::Start)
        .into()
    }

    fn theme(&self) -> Theme {
        Theme::Dark
    }

    fn subscription(&self) -> Subscription<Message> {
        struct Worker;

        enum State {
            Ready,
            Waiting(iced::futures::channel::mpsc::UnboundedReceiver<Message>),
            Finished,
        }

        iced::subscription::unfold(std::any::TypeId::of::<Worker>(), State::Ready, |state| async move {
            match state {
                State::Ready => {
                    let (command_sender, command_receiver) = std::sync::mpsc::channel();

                    let (message_sender, message_receiver) = iced::futures::channel::mpsc::unbounded();

                    //TODO: kill worker thread?
                    let join_handle = std::thread::spawn(move || {
                        loop {
                            let (disk_path, password_opt) = command_receiver.recv().unwrap();
                            println!("Installing to {:?}", disk_path);
                            install(disk_path, password_opt, |message| {
                                message_sender.unbounded_send(message).unwrap();
                            });
                        }
                    });

                    (Some(Message::CommandSender(command_sender)), State::Waiting(message_receiver))
                },
                State::Waiting(mut message_receiver) => {
                    use iced::futures::StreamExt;
                    match message_receiver.next().await {
                        Some(message) => (Some(message), State::Waiting(message_receiver)),
                        None => (None, State::Finished),
                    }
                },
                State::Finished => iced::futures::future::pending().await,
            }
        })
    }
}
