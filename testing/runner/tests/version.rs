//! Checks the `--version` line that harnesses identify the runner by.

use std::process::Command;

/// Runs the runner with `args`, checks that it exited 0 and returns its stdout.
fn stdout(args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_ef-test-runner")).args(args).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn version_is_one_line_naming_the_runner() {
    let expected = format!("ef-test-runner {}", env!("CARGO_PKG_VERSION"));
    for args in [&["--version"][..], &["blocktest", "--version"], &["enginetest", "--version"]] {
        let stdout = stdout(args);
        assert_eq!(stdout.lines().count(), 1, "{args:?}: {stdout:?}");
        assert!(stdout.starts_with(&expected), "{args:?}: {stdout:?}");
    }
}
