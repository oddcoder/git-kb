//! Restarting: `git kb serve` run again in its own process, with the same arguments, once it has stopped; so that it
//! runs as a newer version installed in its place, or reads its settings and plugins again.

use std::path::PathBuf;

/// The binary to run again: where this one was as it began, where a newer one installed over it is too.
#[derive(Debug, Clone)]
pub struct Restart {
    exe: PathBuf,
}

impl Restart {
    /// This binary, where it is as it begins; none when that cannot be told, or on Windows, where a process cannot run
    /// another in its place.
    #[must_use]
    pub fn here() -> Option<Self> {
        if !cfg!(unix) {
            return None;
        }
        std::env::current_exe()
            .and_then(std::fs::canonicalize)
            .ok()
            .map(|exe| Self { exe })
    }

    /// Runs the binary in this process's place, with the same arguments; returns only if it cannot.
    #[cfg(unix)]
    #[must_use]
    pub fn run(&self) -> std::io::Error {
        use std::os::unix::process::CommandExt;
        let mut args = std::env::args_os();
        let mut command = std::process::Command::new(&self.exe);
        if let Some(name) = args.next() {
            command.arg0(name);
        }
        command.args(args).exec()
    }

    /// Never made: see `here`.
    #[cfg(not(unix))]
    #[must_use]
    pub fn run(&self) -> std::io::Error {
        std::io::Error::from(std::io::ErrorKind::Unsupported)
    }
}
