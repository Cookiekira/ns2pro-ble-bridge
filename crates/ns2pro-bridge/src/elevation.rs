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
    let mut command = Command::new(if tty { "sudo" } else { "pkexec" });
    if tty {
        command.arg("--");
    }
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
    fn elevation_preserves_arguments_and_log_filter() {
        for tty in [false, true] {
            for log in [None, Some(OsString::from("ns2pro_bridge=debug,info"))] {
                let command = command(
                    tty,
                    Path::new("/home/user/.cargo/bin/ns2pro-bridge"),
                    ["--cache-file", "/tmp/cache with spaces.json"].map(OsString::from),
                    log.clone(),
                );
                assert_eq!(command.get_program(), if tty { "sudo" } else { "pkexec" });
                let mut expected = Vec::new();
                if tty {
                    expected.push(OsString::from("--"));
                }
                expected.push(OsString::from("/usr/bin/env"));
                if let Some(log) = log {
                    let mut assignment = OsString::from("RUST_LOG=");
                    assignment.push(log);
                    expected.push(assignment);
                }
                expected.extend(
                    [
                        "/home/user/.cargo/bin/ns2pro-bridge",
                        "--cache-file",
                        "/tmp/cache with spaces.json",
                    ]
                    .map(OsString::from),
                );
                assert_eq!(command.get_args().collect::<Vec<_>>(), expected);
            }
        }
    }
}
