use anyhow::format_err;
use cosmic::{
    app::{self, Task},
    iced::{
        self, executor,
        futures::sink::SinkExt,
        widget::{row, text_input},
        window, Alignment, Size, Subscription,
    },
    widget::{button, checkbox, column, progress_bar, radio, space, text},
    Application, ApplicationExt, Core, Element,
};
use futures_channel::mpsc;
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

mod sys;
mod worker;
pub use sys::*;
pub use worker::*;

fn main() -> iced::Result {
    let mut settings = app::Settings::default();
    settings = settings.size(Size::new(700.0, 500.0));
    settings = settings.exit_on_close(false);
    app::run::<Window>(settings, ())
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    Drive,
    Partition,
    Image,
}

#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub struct InstallConfig {
    kind: InstallConfigKind,
    live_disk: bool,
    password_opt: Option<String>,
}

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallConfigKind {
    #[default]
    Desktop,
    Server,
    #[cfg(target_os = "redox")]
    Clone,
}

#[derive(Debug)]
enum Page {
    Begin(Option<TargetKind>),
    DrivePart {
        kind: TargetKind,
        selected_disk_i: Option<usize>,
    },
    ImageConfig {
        path: String,
        size: String,
        skip_partition: bool,
    },
    Profile {
        target: TargetConfig,
        selected: InstallConfig,
    },
    Sudo(String),
    Install(usize, String),
    Success(String),
    Error(String),
}

#[derive(Clone, Debug)]
struct Worker {
    command_sender: std::sync::mpsc::Sender<(TargetConfig, InstallConfig)>,
    join_handle: Arc<std::thread::JoinHandle<()>>,
}

#[derive(Clone, Debug)]
enum Message {
    None,
    Worker(Worker),
    SudoInput(String),
    SudoSubmit,

    BeginChoose(TargetKind),
    BeginConfirm,
    DrivePartChoose(usize),
    DrivePartConfirm,
    ImagePathInput(String),
    ImageSizeInput(String),
    ImageBrowse,
    ImageBrowseResult(Option<String>),
    ImageConfirm,
    ProfileChoose(InstallConfigKind),
    ProfileChooseLive(bool),
    ProfileChoosePassword(bool),
    ProfileEnterPassword(String),
    ProfileConfirm,
    GoBack,

    Install(usize, String),
    Success(String),
    Exit,
    Error(String),
}

struct Window {
    core: Core,
    page: Page,
    disk_paths: Vec<(String, bool, u64)>,
    worker_opt: Option<Worker>,
}

enum State {
    Ready,
    Waiting(mpsc::UnboundedReceiver<Message>),
    Finished,
}

impl Window {
    fn worker_stream() -> impl iced::futures::Stream<Item = Message> {
        iced::stream::channel(100, |mut output: mpsc::Sender<Message>| async move {
            let mut state = State::Ready;
            loop {
                let (message, new_state) = match state {
                    State::Ready => {
                        let (command_sender, command_receiver) = std::sync::mpsc::channel();
                        let (message_sender, message_receiver) = mpsc::unbounded();

                        let join_handle = std::thread::spawn(move || {
                            while let Ok((target, profile)) = command_receiver.recv() {
                                install(target, profile, |message| {
                                    message_sender.unbounded_send(message).unwrap();
                                });
                            }
                        });

                        let worker = Worker {
                            command_sender,
                            join_handle: std::sync::Arc::new(join_handle),
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
        })
    }
}

impl Application for Window {
    type Executor = executor::Default;
    type Flags = ();
    type Message = Message;

    const APP_ID: &'static str = "org.redox-os.InstallerGui";

    fn init(core: Core, _flags: ()) -> (Self, Task<Message>) {
        let (page, disk_paths) = match disk_paths() {
            Ok(disk_paths) => (Page::Begin(None), disk_paths),
            Err(err) => (Page::Error(err), Vec::new()),
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

    fn header_start(&self) -> Vec<Element<'_, Self::Message>> {
        let mut elements = Vec::new();

        let can_go_back = matches!(
            self.page,
            Page::DrivePart { .. } | Page::ImageConfig { .. } | Page::Profile { .. }
        );

        if can_go_back {
            elements.push(
                cosmic::widget::button::standard("Back")
                    .on_press(Message::GoBack)
                    .into(),
            );
        }

        elements.push(
            cosmic::widget::header_bar()
                .title("Redox OS Installer")
                .into(),
        );

        elements
    }
    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::None => {}
            Message::Worker(worker) => {
                self.worker_opt = Some(worker);
            }

            Message::BeginChoose(kind) => {
                self.page = Page::Begin(Some(kind));
            }
            Message::BeginConfirm => {
                if let Page::Begin(Some(kind)) = self.page {
                    self.page = match kind {
                        TargetKind::Drive | TargetKind::Partition => Page::DrivePart {
                            kind,
                            selected_disk_i: None,
                        },
                        TargetKind::Image => Page::ImageConfig {
                            path: String::new(),
                            size: "1024".to_string(),
                            skip_partition: false,
                        },
                    };
                }
            }
            Message::DrivePartChoose(disk_i) => {
                if let Page::DrivePart { kind, .. } = self.page {
                    self.page = Page::DrivePart {
                        kind,
                        selected_disk_i: Some(disk_i),
                    };
                }
            }
            Message::DrivePartConfirm => {
                if let Page::DrivePart {
                    selected_disk_i: Some(i),
                    ..
                } = self.page
                {
                    if let Some((disk_path, _, disk_size)) = self.disk_paths.get(i) {
                        self.page = Page::Profile {
                            target: TargetConfig::Disk((disk_path.clone(), *disk_size)),
                            selected: InstallConfig::default(),
                        };
                    }
                }
            }
            Message::ImagePathInput(val) => {
                if let Page::ImageConfig { ref mut path, .. } = self.page {
                    *path = val;
                }
            }
            Message::ImageSizeInput(val) => {
                if let Page::ImageConfig { ref mut size, .. } = self.page {
                    *size = val;
                }
            }
            Message::ImageBrowse => {
                #[cfg(target_os = "linux")]
                return Task::perform(
                    async {
                        rfd::AsyncFileDialog::new()
                            .set_title("Save Image File")
                            .save_file()
                            .await
                            .map(|f| f.path().to_str().unwrap().to_string())
                    },
                    |result| Message::ImageBrowseResult(result).into(),
                );
                #[cfg(not(target_os = "linux"))]
                unreachable!()
            }
            Message::ImageBrowseResult(Some(new_path)) => {
                if let Page::ImageConfig { ref mut path, .. } = self.page {
                    *path = new_path;
                }
            }
            Message::ImageBrowseResult(None) => {}
            Message::ImageConfirm => {
                if let Page::ImageConfig {
                    path,
                    size,
                    skip_partition,
                } = &self.page
                {
                    match size.parse::<u64>() {
                        Ok(size_mb) if !path.is_empty() => {
                            self.page = Page::Profile {
                                target: TargetConfig::Image {
                                    path: path.clone(),
                                    size_mb,
                                    skip_partition: *skip_partition,
                                },
                                selected: InstallConfig::default(),
                            };
                        }
                        _ => {
                            // TODO: validation error
                        }
                    }
                }
            }
            Message::ProfileChoose(profile) => {
                if let Page::Profile { selected, .. } = &mut self.page {
                    selected.kind = profile;
                }
            }
            Message::ProfileChooseLive(live_disk) => {
                if let Page::Profile { selected, .. } = &mut self.page {
                    selected.live_disk = live_disk;
                }
            }
            Message::ProfileChoosePassword(password) => {
                if let Page::Profile { selected, .. } = &mut self.page {
                    selected.password_opt = if password { Some("".to_string()) } else { None };
                }
            }
            Message::ProfileEnterPassword(password) => {
                if let Page::Profile { selected, .. } = &mut self.page {
                    selected.password_opt = Some(password);
                }
            }
            Message::ProfileConfirm => {
                if let Page::Profile {
                    target,
                    selected: profile,
                } = &self.page
                {
                    if let Some(worker) = &self.worker_opt {
                        match worker
                            .command_sender
                            .send((target.clone(), profile.clone()))
                        {
                            Ok(()) => self.page = Page::Install(0, format!("Starting install...")),
                            Err(err) => {
                                self.page = Page::Error(format!("failed to send command: {}", err));
                            }
                        }
                    }
                }
            }
            Message::GoBack => match &self.page {
                Page::DrivePart { kind, .. } => {
                    self.page = Page::Begin(Some(*kind));
                }
                Page::ImageConfig { .. } => {
                    self.page = Page::Begin(Some(TargetKind::Image));
                }
                Page::Profile { target, .. } => {
                    self.page = match target {
                        TargetConfig::Disk((disk_path, _)) => {
                            let selected_disk_i =
                                self.disk_paths.iter().position(|(p, _, _)| p == disk_path);
                            Page::DrivePart {
                                kind: TargetKind::Drive,
                                selected_disk_i,
                            }
                        }
                        TargetConfig::Partition((disk_path, _)) => {
                            let selected_disk_i =
                                self.disk_paths.iter().position(|(p, _, _)| p == disk_path);
                            Page::DrivePart {
                                kind: TargetKind::Partition,
                                selected_disk_i,
                            }
                        }
                        TargetConfig::Image {
                            path,
                            size_mb,
                            skip_partition,
                        } => Page::ImageConfig {
                            path: path.clone(),
                            size: size_mb.to_string(),
                            skip_partition: *skip_partition,
                        },
                    };
                }
                _ => unreachable!(),
            },

            Message::SudoInput(password) => {
                self.page = Page::Sudo(password);
            }
            Message::SudoSubmit => {
                #[cfg(target_os = "redox")]
                if let Page::Sudo(password) = &self.page {
                    match ask_root(&password) {
                        Ok(()) => self.page = Page::Begin(None),
                        Err(err) => eprintln!("{err}"),
                    }
                }
                #[cfg(target_os = "linux")]
                match ask_root() {
                    Ok(()) => {
                        if let Some(window_id) = self.core.main_window_id() {
                            return window::close(window_id);
                        }
                    }
                    Err(err) => eprintln!("{err}"),
                }
            }
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

    fn view(&self) -> Element<'_, Message> {
        let mut widgets = Vec::new();
        match &self.page {
            Page::Begin(target_opt) => {
                widgets.push(
                    text("Where do you want to install Redox OS?")
                        .size(24)
                        .into(),
                );
                widgets.push(
                    radio(
                        "Whole Drive",
                        TargetKind::Drive,
                        *target_opt,
                        Message::BeginChoose,
                    )
                    .into(),
                );
                widgets.push(
                    radio(
                        "Specific Partition",
                        TargetKind::Partition,
                        *target_opt,
                        Message::BeginChoose,
                    )
                    .into(),
                );
                widgets.push(
                    radio(
                        "Image File",
                        TargetKind::Image,
                        *target_opt,
                        Message::BeginChoose,
                    )
                    .into(),
                );

                if target_opt.is_some() {
                    widgets.push(space::vertical().into());
                    widgets.push(
                        button::suggested("Next")
                            .on_press(Message::BeginConfirm)
                            .into(),
                    );
                }
            }
            Page::DrivePart {
                kind,
                selected_disk_i,
            } => {
                let is_part = *kind == TargetKind::Partition;
                widgets.push(
                    text(if is_part {
                        "Choose a partition:"
                    } else {
                        "Choose a drive:"
                    })
                    .size(24)
                    .into(),
                );

                if !self.disk_paths.is_empty() {
                    for (disk_i, (disk_path, is_partition, disk_size)) in
                        self.disk_paths.iter().enumerate()
                    {
                        if *is_partition == is_part {
                            widgets.push(
                                row![
                                    radio(
                                        text(disk_path),
                                        disk_i,
                                        *selected_disk_i,
                                        Message::DrivePartChoose
                                    ),
                                    space::horizontal(),
                                    text(redox_installer::format_bytes(*disk_size)),
                                ]
                                .into(),
                            );
                        }
                    }

                    if selected_disk_i.is_some() && is_root() {
                        widgets.push(space::vertical().into());
                        widgets.push(
                            row![
                                space::horizontal(),
                                button::suggested("Next").on_press(Message::DrivePartConfirm),
                            ]
                            .into(),
                        );
                    }
                } else {
                    widgets.push(text("No matching devices found").into());
                }

                if !is_root() {
                    #[cfg(target_os = "linux")]
                    let page = Message::SudoSubmit;
                    #[cfg(target_os = "redox")]
                    let page = Message::SudoInput(String::new());

                    widgets.push(space::vertical().into());
                    widgets.push(
                        row![
                            text("Superuser permission is required to install to devices."),
                            space::horizontal(),
                            button::suggested("Ask root access").on_press(page),
                        ]
                        .into(),
                    );
                }
            }
            Page::ImageConfig {
                path,
                size,
                skip_partition,
            } => {
                widgets.push(text("Configure Image File").size(24).into());
                widgets.push(text("Image Path:").into());
                widgets.push(
                    row![
                        text_input("e.g. redox.img", path).on_input(Message::ImagePathInput),
                        if cfg!(target_os = "linux") {
                            button::standard("Browse").on_press(Message::ImageBrowse)
                        } else {
                            button::standard("")
                        },
                    ]
                    .spacing(8)
                    .into(),
                );

                widgets.push(text("Image Size (MB):").into());
                widgets.push(
                    text_input("Size in MB (e.g. 1024)", size)
                        .on_input(Message::ImageSizeInput)
                        .into(),
                );

                widgets.push(checkbox(*skip_partition).label("Skip Partition?").into());

                if !path.is_empty() && size.parse::<u64>().is_ok() {
                    widgets.push(space::vertical().into());
                    widgets.push(
                        button::suggested("Next")
                            .on_press(Message::ImageConfirm)
                            .into(),
                    );
                }
            }
            Page::Profile { selected, .. } => {
                widgets.push(text("Select System Profile").size(24).into());
                #[cfg(target_os = "redox")]
                {
                    widgets.push(
                        radio(
                            "Clone this OS",
                            InstallConfigKind::Clone,
                            Some(selected.kind),
                            Message::ProfileChoose,
                        )
                        .into(),
                    );
                }
                widgets.push(
                    radio(
                        "Desktop",
                        InstallConfigKind::Desktop,
                        Some(selected.kind),
                        Message::ProfileChoose,
                    )
                    .into(),
                );
                widgets.push(
                    radio(
                        "Server",
                        InstallConfigKind::Server,
                        Some(selected.kind),
                        Message::ProfileChoose,
                    )
                    .into(),
                );
                widgets.push(
                    checkbox(selected.live_disk)
                        .label("Install as live disk")
                        .on_toggle(Message::ProfileChooseLive)
                        .into(),
                );

                widgets.push(
                    checkbox(selected.password_opt.is_some())
                        .label("Enable disk encryption password")
                        .on_toggle(Message::ProfileChoosePassword)
                        .into(),
                );
                match selected.password_opt.as_ref() {
                    Some(pass) => widgets.push(
                        text_input("", pass)
                            .on_input(Message::ProfileEnterPassword)
                            .secure(true)
                            .into(),
                    ),
                    None => {}
                }

                widgets.push(space::vertical().into());
                widgets.push(
                    button::destructive("Start Installation")
                        .on_press(Message::ProfileConfirm)
                        .into(),
                );
            }
            Page::Sudo(password) => {
                widgets.push(text("Enter your password:").into());
                widgets.push(
                    text_input("", password)
                        .on_input(Message::SudoInput)
                        .secure(true)
                        .on_submit(Message::SudoSubmit)
                        .into(),
                );
            }
            Page::Install(progress, description) => {
                widgets.push(text("Installation progress:").size(24).into());
                widgets.push(progress_bar::determinate_linear(*progress as f32 / 100.).into());
                widgets.push(text(description).into());
            }
            Page::Success(description) => {
                widgets.push(text("Installation complete!").size(24).into());
                widgets.push(text(description).into());
                widgets.push(space::vertical().into());
                widgets.push(
                    row![
                        space::horizontal(),
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
            .spacing(12)
            .padding(24)
            .align_x(Alignment::Start)
            .into()
    }

    fn subscription(&self) -> Subscription<Message> {
        Subscription::run(Self::worker_stream)
    }
}
