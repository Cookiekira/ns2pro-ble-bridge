use std::process::Command;

#[test]
fn help_version_and_bad_arguments_exit_before_elevation() {
    for (argument, code, expected) in [
        ("--help", 0, "Usage: ns2pro-bridge"),
        (
            "--version",
            0,
            concat!("ns2pro-bridge ", env!("CARGO_PKG_VERSION")),
        ),
        ("--unknown-option", 2, "unexpected argument"),
    ] {
        // No elevation programs are available, and piped stdin is not a TTY.
        let output = Command::new(env!("CARGO_BIN_EXE_ns2pro-bridge"))
            .arg(argument)
            .env("PATH", "")
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(code));
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(text.contains(expected), "{text}");
        assert!(!text.contains("could not elevate privileges"), "{text}");
    }
}
