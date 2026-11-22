//! The platform layer: one API, implemented with Win32 Job Objects on Windows and process
//! groups and signals on macOS and Linux. Everything is declared by hand (no bindings crates).
//!
//! * `Job`: a managed process and everything it starts, ended together.
//! * `prepare_child`, `kill_tree`, `spawn_detached`: starting and stopping processes.
//! * `free_bytes`, `process_memory`, `pid_alive`, `run_elevated`, `is_elevated`, `open_url`.

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::*;

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::*;

use std::process::Command;

/// The platform's executable suffix: ".exe" on Windows, "" elsewhere.
pub const EXE: &str = std::env::consts::EXE_SUFFIX;

/// `name` with the platform's executable suffix ("php" becomes "php.exe" on Windows).
pub fn exe(name: &str) -> String { format!("{name}{EXE}") }

/// "windows", "macos" or "linux".
pub fn os() -> &'static str { std::env::consts::OS }

/// "x86_64" or "aarch64".
pub fn arch() -> &'static str { std::env::consts::ARCH }

/// Commands tez runs for itself (php -v, gh, reg): no console window flashing up on Windows.
pub trait Hidden {
    fn hidden(&mut self) -> &mut Self;
}

impl Hidden for Command {
    fn hidden(&mut self) -> &mut Self {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            self.creation_flags(CREATE_NO_WINDOW);
        }
        self
    }
}

/// The separator between entries of PATH: ";" on Windows, ":" elsewhere.
pub const PATH_SEP: &str = if cfg!(windows) { ";" } else { ":" };

/// Line endings for text files tez edits that other programs share (hosts, .env).
pub const NL: &str = if cfg!(windows) { "\r\n" } else { "\n" };
