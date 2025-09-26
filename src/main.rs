use anyhow::format_err;
use cosmic::{
    app::{self, Task},
    iced::{
        self, executor, futures::sink::SinkExt, stream, widget::row, window, Alignment, Size,
        Subscription,
    },
    widget::{
        button, column, horizontal_space, progress_bar, radio, text, text_input, vertical_space,
    },
    Application, ApplicationExt, Core, Element,
};
use pkgar::{ext::EntryExt, PackageHead};
use pkgar_core::PackageSrc;
use pkgar_keys::PublicKeyFile;
use redox_installer::{try_fast_install, with_redoxfs_mount, with_whole_disk, Config, DiskOption};
use std::{
    ffi::OsStr,
    fs,
    io::{self, Read, Write},
    os::unix::fs::{symlink, MetadataExt, OpenOptionsExt},
    path::Path,
    sync::Arc,
};

fn main() -> iced::Result {
    let mut settings = app::Settings::default();
    settings = settings.size(Size::new(608.0, 416.0));
    settings = settings.exit_on_close(false);
    app::run::<Window>(settings, ())
}

fn sudo(password: &str) -> Result<(), String> {
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

fn disk_paths() -> Result<Vec<(String, u64)>, String> {
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
        if is_dir {
            match fs::read_dir(&scheme) {
                Ok(entries) => {
                    for entry_res in entries {
                        if let Ok(entry) = entry_res {
                            if let Ok(file_name) = entry.file_name().into_string() {
                                if file_name.contains('p') {
                                    // Skip partitions
                                    continue;
                                }

                                if let Ok(path) = entry.path().into_os_string().into_string() {
                                    if let Ok(metadata) = entry.metadata() {
                                        let size = metadata.len();
                                        if size > 0 {
                                            paths.push((path, size));
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
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

fn copy_file(src: &Path, dest: &Path, buf: &mut [u8]) -> anyhow::Result<()> {
    if let Some(parent) = dest.parent() {
        // Parent may be a symlink
        if !parent.is_symlink() {
            match fs::create_dir_all(&parent) {
                Ok(()) => (),
                Err(err) => {
                    return Err(format_err!(
                        "failed to create directory {}: {}",
                        parent.display(),
                        err
                    ));
                }
            }
        }
    }

    let metadata = match fs::symlink_metadata(&src) {
        Ok(ok) => ok,
        Err(err) => {
            return Err(format_err!(
                "failed to read metadata of {}: {}",
                src.display(),
                err
            ));
        }
    };

    if metadata.file_type().is_symlink() {
        let real_src = match fs::read_link(&src) {
            Ok(ok) => ok,
            Err(err) => {
                return Err(format_err!(
                    "failed to read link {}: {}",
                    src.display(),
                    err
                ));
            }
        };

        match symlink(&real_src, &dest) {
            Ok(()) => (),
            Err(err) => {
                return Err(format_err!(
                    "failed to copy link {} ({}) to {}: {}",
                    src.display(),
                    real_src.display(),
                    dest.display(),
                    err
                ));
            }
        }
    } else {
        let mut src_file = match fs::File::open(&src) {
            Ok(ok) => ok,
            Err(err) => {
                return Err(format_err!(
                    "failed to open file {}: {}",
                    src.display(),
                    err
                ));
            }
        };

        let mut dest_file = match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(metadata.mode())
            .open(&dest)
        {
            Ok(ok) => ok,
            Err(err) => {
                return Err(format_err!(
                    "failed to create file {}: {}",
                    dest.display(),
                    err
                ));
            }
        };

        loop {
            let count = match src_file.read(buf) {
                Ok(ok) => ok,
                Err(err) => {
                    return Err(format_err!(
                        "failed to read file {}: {}",
                        src.display(),
                        err
                    ));
                }
            };

            if count == 0 {
                break;
            }

            match dest_file.write_all(&buf[..count]) {
                Ok(()) => (),
                Err(err) => {
                    return Err(format_err!(
                        "failed to write file {}: {}",
                        dest.display(),
                        err
                    ));
                }
            }
        }
    }

    Ok(())
}

fn package_files(
    root_path: &Path,
    config: &mut Config,
    files: &mut Vec<String>,
) -> Result<(), anyhow::Error> {
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
                files.push(entry.check_path()?.to_str().unwrap().to_string());
            }
            files.push(
                pkg_path
                    .strip_prefix(root_path)
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_string(),
            );
        }
    }

    Ok(())
}

fn install<F: FnMut(Message)>(disk_path: String, password_opt: Option<String>, mut f: F) {
    let start = std::time::Instant::now();

    let mut progress = 0;

    macro_rules! message {
        ($($arg:tt)*) => {{
            eprintln!($($arg)*);
            f(Message::Install(
                progress,
                format!($($arg)*)
            ));
        }}
    }

    let root_path = Path::new("/scheme/file/");

    message!("Loading bootloader");
    let bootloader_bios = {
        let path = root_path.join("boot").join("bootloader.bios");
        if path.exists() {
            match fs::read(&path) {
                Ok(ok) => ok,
                Err(err) => {
                    f(Message::Error(format!(
                        "{}: failed to read: {}",
                        path.display(),
                        err
                    )));
                    return;
                }
            }
        } else {
            Vec::new()
        }
    };

    message!("Loading bootloader.efi");
    let bootloader_efi = {
        let path = root_path.join("boot").join("bootloader.efi");
        if path.exists() {
            match fs::read(&path) {
                Ok(ok) => ok,
                Err(err) => {
                    f(Message::Error(format!(
                        "{}: failed to read: {}",
                        path.display(),
                        err
                    )));
                    return;
                }
            }
        } else {
            Vec::new()
        }
    };

    message!("Formatting disk");
    let disk_option = DiskOption {
        bootloader_bios: &bootloader_bios,
        bootloader_efi: &bootloader_efi,
        password_opt: password_opt.as_ref().map(|x| x.as_bytes()),
        efi_partition_size: None,
        skip_partitions: false,
    };
    let res = with_whole_disk(&disk_path, &disk_option, |mut fs| -> anyhow::Result<()> {
        // Fast install method via filesystem clone
        let mut last_progress = 0;
        if try_fast_install(&mut fs, |used, used_old| {
            progress = ((used * 100) / used_old) as usize;
            if progress != last_progress {
                message!(
                    "{}%: {} MB/{} MB",
                    progress,
                    used / 1000 / 1000,
                    used_old / 1000 / 1000
                );
                last_progress = progress;
            }
        })? {
            progress = 100;
            message!("Finished installing using fast mode");
            return Ok(());
        }

        with_redoxfs_mount(fs, |mount_path: &Path| -> anyhow::Result<()> {
            message!("Loading filesystem.toml");
            let mut config: Config = {
                let path = root_path.join("filesystem.toml");
                match fs::read_to_string(&path) {
                    Ok(config_data) => match toml::from_str(&config_data) {
                        Ok(config) => config,
                        Err(err) => {
                            return Err(format_err!(
                                "{}: failed to decode: {}",
                                path.display(),
                                err
                            ));
                        }
                    },
                    Err(err) => {
                        return Err(format_err!("{}: failed to read: {}", path.display(), err));
                    }
                }
            };

            // Copy filesystem.toml, which is not packaged
            let mut files = vec!["filesystem.toml".to_string()];

            // Copy files from locally installed packages
            message!("Loading package files");
            if let Err(err) = package_files(&root_path, &mut config, &mut files) {
                return Err(format_err!("failed to read package files: {}", err));
            }

            // Sort and remove duplicates
            files.sort();
            files.dedup();

            // Perform config install (after packages have been converted to files)
            message!("Configuring system");
            let cookbook: Option<&'static str> = None;
            redox_installer::install_dir(config, mount_path, cookbook)
                .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;

            // Install files
            let mut buf = vec![0; 4 * MIB as usize];
            for (i, name) in files.iter().enumerate() {
                progress = (i * 100) / files.len();
                message!("Copy {} [{}/{}]", name, i, files.len());

                let src = root_path.join(name);
                let dest = mount_path.join(name);
                copy_file(&src, &dest, &mut buf)?;
            }

            progress = 100;
            message!("Finished installing, unmounting filesystem");
            Ok(())
        })
    });

    match res {
        Ok(()) => {
            f(Message::Success(format!(
                "Finished installing in {:?}, ready to reboot",
                start.elapsed()
            )));
        }
        Err(err) => {
            f(Message::Error(format!("Failed to install: {}", err)));
        }
    }
}

#[derive(Debug)]
enum Page {
    Sudo(String),
    Disk(Option<usize>),
    Install(usize, String),
    Success(String),
    Error(String),
}

#[derive(Clone, Debug)]
struct Worker {
    command_sender: std::sync::mpsc::Sender<(String, Option<String>)>,
    join_handle: Arc<std::thread::JoinHandle<()>>,
}

#[derive(Clone, Debug)]
enum Message {
    None,
    Worker(Worker),
    SudoInput(String),
    SudoSubmit,
    DiskChoose(usize),
    DiskConfirm(usize),
    Install(usize, String),
    Success(String),
    Exit,
    Error(String),
}

struct Window {
    core: Core,
    page: Page,
    disk_paths: Vec<(String, u64)>,
    worker_opt: Option<Worker>,
}

impl Application for Window {
    type Executor = executor::Default;
    type Flags = ();
    type Message = Message;

    const APP_ID: &'static str = "org.redox-os.InstallerGui";

    fn init(core: Core, _flags: ()) -> (Self, Task<Message>) {
        let uid = libredox::call::geteuid().unwrap();
        let (page, disk_paths) = if uid == 0 {
            //TODO: load in background
            match disk_paths() {
                Ok(disk_paths) => (Page::Disk(None), disk_paths),
                Err(err) => (Page::Error(err), Vec::new()),
            }
        } else {
            (Page::Sudo(String::new()), Vec::new())
        };

        let mut app = Self {
            core,
            page,
            disk_paths,
            worker_opt: None,
        };
        let task = app.set_window_title("Redox OS Installer".to_string());
        (app, task)
    }

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::None => {}
            Message::Worker(worker) => {
                self.worker_opt = Some(worker);
            }
            Message::SudoInput(password) => {
                self.page = Page::Sudo(password);
            }
            Message::SudoSubmit => {
                if let Page::Sudo(password) = &self.page {
                    //TODO: run async?
                    match sudo(password) {
                        Ok(()) => {
                            (self.page, self.disk_paths) = match disk_paths() {
                                Ok(disk_paths) => (Page::Disk(None), disk_paths),
                                Err(err) => (Page::Error(err), Vec::new()),
                            };
                        }
                        Err(err) => {
                            //TODO: show error in GUI
                            eprintln!("{err}");
                            self.page = Page::Sudo(String::new());
                        }
                    }
                }
            }
            Message::DiskChoose(disk_i) => {
                self.page = Page::Disk(Some(disk_i));
            }
            Message::DiskConfirm(disk_i) => match self.disk_paths.get(disk_i) {
                Some((disk_path, _disk_size)) => match &self.worker_opt {
                    Some(worker) => match worker.command_sender.send((disk_path.clone(), None)) {
                        Ok(()) => self.page = Page::Install(0, format!("Starting install...")),
                        Err(err) => {
                            self.page = Page::Error(format!("failed to send command: {}", err));
                        }
                    },
                    None => {
                        self.page = Page::Error(format!("command sender not found"));
                    }
                },
                None => {
                    self.page = Page::Error(format!("invalid disk number {} chosen", disk_i));
                }
            },
            Message::Install(progress, description) => {
                self.page = Page::Install(progress, description);
            }
            Message::Success(description) => {
                self.page = Page::Success(description);
            }
            Message::Error(err) => {
                self.page = Page::Error(err);
            }
            Message::Exit => {
                if let Some(worker) = self.worker_opt.take() {
                    drop(worker.command_sender);
                    let join_handle = Arc::try_unwrap(worker.join_handle).unwrap();
                    join_handle.join().unwrap();
                }
                if let Some(window_id) = self.core.main_window_id() {
                    return window::close(window_id);
                }
            }
        }
        Task::none()
    }

    fn view(&self) -> Element<Message> {
        let mut widgets = Vec::new();
        match &self.page {
            Page::Sudo(password) => {
                widgets.push(text("Enter your password:").into());
                widgets.push(
                    text_input("", password)
                        .password()
                        .on_input(Message::SudoInput)
                        .on_submit(|_| Message::SudoSubmit)
                        .into(),
                );
            }
            Page::Disk(disk_i_opt) => {
                if !self.disk_paths.is_empty() {
                    widgets.push(text("Choose a drive:").size(24).into());

                    for (disk_i, (disk_path, disk_size)) in self.disk_paths.iter().enumerate() {
                        widgets.push(
                            row![
                                radio(text(disk_path), disk_i, *disk_i_opt, Message::DiskChoose),
                                horizontal_space(),
                                text(format_size(*disk_size)),
                            ]
                            .into(),
                        );
                    }

                    if let Some(disk_i) = *disk_i_opt {
                        widgets.push(vertical_space().into());
                        widgets.push(
                            row![
                                horizontal_space(),
                                button::destructive("Confirm")
                                    .on_press(Message::DiskConfirm(disk_i)),
                            ]
                            .into(),
                        );
                    }
                } else {
                    widgets.push(text("No drives found").into());
                }
            }
            Page::Install(progress, description) => {
                widgets.push(text("Installation progress:").size(24).into());
                widgets.push(progress_bar(0.0..=100.0, *progress as f32).into());
                widgets.push(text(description).into());
            }
            Page::Success(description) => {
                widgets.push(text("Installation complete!").size(24).into());
                widgets.push(text(description).into());
                widgets.push(vertical_space().into());
                widgets.push(
                    row![
                        horizontal_space(),
                        button::standard("Exit").on_press(Message::Exit),
                    ]
                    .into(),
                );
            }
            Page::Error(err) => {
                widgets.push(text(format!("{}", err)).into());
            }
        };

        column::with_children(widgets)
            .spacing(8)
            .padding(24)
            .align_x(Alignment::Start)
            .into()
    }

    fn subscription(&self) -> Subscription<Message> {
        enum State {
            Ready,
            Waiting(iced::futures::channel::mpsc::UnboundedReceiver<Message>),
            Finished,
        }

        Subscription::run_with_id(
            std::any::TypeId::of::<Worker>(),
            stream::channel(100, |mut output| async move {
                let mut state = State::Ready;
                loop {
                    let (message, new_state) = match state {
                        State::Ready => {
                            let (command_sender, command_receiver) = std::sync::mpsc::channel();

                            let (message_sender, message_receiver) =
                                iced::futures::channel::mpsc::unbounded();

                            //TODO: kill worker thread?
                            let join_handle = std::thread::spawn(move || {
                                while let Ok((disk_path, password_opt)) = command_receiver.recv() {
                                    println!("Installing to {:?}", disk_path);
                                    install(disk_path, password_opt, |message| {
                                        message_sender.unbounded_send(message).unwrap();
                                    });
                                }
                            });

                            let worker = Worker {
                                command_sender,
                                join_handle: Arc::new(join_handle),
                            };

                            (Message::Worker(worker), State::Waiting(message_receiver))
                        }
                        State::Waiting(mut message_receiver) => {
                            use iced::futures::StreamExt;
                            match message_receiver.next().await {
                                Some(message) => (message, State::Waiting(message_receiver)),
                                None => (Message::None, State::Finished),
                            }
                        }
                        State::Finished => iced::futures::future::pending().await,
                    };
                    output.send(message).await.unwrap();
                    state = new_state;
                }
            }),
        )
    }
}
