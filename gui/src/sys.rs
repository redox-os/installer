#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "redox")]
mod redox;

#[cfg(target_os = "linux")]
pub use linux::*;
#[cfg(target_os = "redox")]
pub use redox::*;

#[cfg(not(any(target_os = "linux", target_os = "redox")))]
compile_error!("Platform is not ported");
