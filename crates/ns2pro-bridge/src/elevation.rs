//! Replace an unprivileged Linux process with the same invocation as root.
use std::{
    ffi::OsString,
    io::{self, IsTerminal},
    os::unix::process::CommandExt,
    path::Path,
    process::Command,
};

fn command(
    euid: u32,
    tty: bool,
    executable: &Path,
    args: impl IntoIterator<Item = OsString>,
    rust_log: Option<OsString>,
) -> Option<Command> {
    if euid == 0 {
        return None;
    }
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
    Some(command)
}

pub fn elevate() -> io::Result<()> {
    let euid = unsafe { libc::geteuid() };
    if euid == 0 {
        return Ok(());
    }
    let mut command = command(
        euid,
        io::stdin().is_terminal(),
        &std::env::current_exe()?,
        std::env::args_os().skip(1),
        std::env::var_os("RUST_LOG"),
    )
    .expect("non-root invocation needs elevation");
    Err(command.exec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_never_elevates() {
        for tty in [false, true] {
            assert!(command(0, tty, Path::new("/bridge"), [], None).is_none());
        }
    }

    #[test]
    fn elevation_preserves_arguments_and_log_filter() {
        for tty in [false, true] {
            for log in [None, Some(OsString::from("ns2pro_bridge=debug,info"))] {
                let command = command(
                    1000,
                    tty,
                    Path::new("/home/user/.cargo/bin/ns2pro-bridge"),
                    ["--cache-file", "/tmp/cache with spaces.json"].map(OsString::from),
                    log.clone(),
                )
                .unwrap();
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
