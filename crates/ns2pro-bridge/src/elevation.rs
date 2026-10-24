//! Replace an unprivileged Linux process with the same invocation as root.
use std::{
    ffi::OsString,
    io::{self, IsTerminal},
    os::unix::process::CommandExt,
    path::Path,
    process::Command,
};

fn command(
    tty: bool,
    executable: &Path,
    args: impl IntoIterator<Item = OsString>,
    rust_log: Option<OsString>,
) -> Command {
    let mut command = if tty {
        let mut command = Command::new("sudo");
        command.arg("--");
        command
    } else {
        Command::new("pkexec")
    };
    command.arg("/usr/bin/env");
    if let Some(value) = rust_log {
        let mut assignment = OsString::from("RUST_LOG=");
        assignment.push(value);
        command.arg(assignment);
    }
    command.arg(executable).args(args);
    command
}

pub fn elevate() -> io::Result<()> {
    if unsafe { libc::geteuid() } == 0 {
        return Ok(());
    }
    Err(command(
        io::stdin().is_terminal(),
        &std::env::current_exe()?,
        std::env::args_os().skip(1),
        std::env::var_os("RUST_LOG"),
    )
    .exec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tty_uses_sudo_and_preserves_log_filter() {
        let command = command(
            true,
            Path::new("/x"),
            [OsString::from("--a")],
            Some(OsString::from("debug")),
        );
        assert_eq!(command.get_program(), "sudo");
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            ["--", "/usr/bin/env", "RUST_LOG=debug", "/x", "--a"]
        );
    }

    #[test]
    fn no_tty_uses_pkexec_and_preserves_log_filter() {
        let command = command(
            false,
            Path::new("/x"),
            [OsString::from("--a")],
            Some(OsString::from("debug")),
        );
        assert_eq!(command.get_program(), "pkexec");
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            ["/usr/bin/env", "RUST_LOG=debug", "/x", "--a"]
        );
    }

    #[test]
    fn missing_log_filter_is_omitted_and_spaces_stay_in_one_argument() {
        let command = command(
            true,
            Path::new("/home/user/.cargo/bin/ns2pro-bridge"),
            ["--cache-file", "/tmp/cache with spaces.json"].map(OsString::from),
            None,
        );
        assert_eq!(command.get_program(), "sudo");
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            [
                "--",
                "/usr/bin/env",
                "/home/user/.cargo/bin/ns2pro-bridge",
                "--cache-file",
                "/tmp/cache with spaces.json"
            ]
        );
    }
}
