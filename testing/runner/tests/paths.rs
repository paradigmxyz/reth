//! Checks that every path given to the runner runs, in one process with one results array.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

/// A Paris fixture with one block on genesis, in the given format.
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

/// Runs `command` on one fixture file and on a directory holding the other, and checks that
/// both fixtures' results come back in one array.
fn check(command: &str, extra_args: &[&str], file: &str, file_in_dir: &str) {
    let dir = tempfile::tempdir().unwrap();
    fs::copy(fixture(file_in_dir), dir.path().join(file_in_dir)).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_ef-test-runner"))
        .arg(command)
        .arg(fixture(file))
        .arg(dir.path())
        .args(extra_args)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));

    let results: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).unwrap();
    let mut names: Vec<_> = results.iter().map(|result| result["name"].as_str().unwrap()).collect();
    names.sort_unstable();
    assert_eq!(names, ["bad_state_root", "empty_block"]);
    assert!(results.iter().all(|result| result["pass"] == true), "{results:?}");
}

#[test]
fn blocktest_runs_every_path() {
    check("blocktest", &[], "blocktest_empty_block.json", "blocktest_bad_state_root.json");
}

#[test]
fn enginetest_runs_every_path() {
    check(
        "enginetest",
        &["--workers", "1"],
        "enginetest_empty_block.json",
        "enginetest_bad_state_root.json",
    );
}

/// A path that does not exist stops the run before any fixture runs.
#[test]
fn a_missing_path_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_ef-test-runner"))
        .arg("blocktest")
        .arg(fixture("blocktest_empty_block.json"))
        .arg(dir.path().join("missing.json"))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty(), "{}", String::from_utf8_lossy(&output.stdout));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("cannot read") && stderr.contains("missing.json"), "{stderr}");
}

/// A fixture file found in a directory that cannot be read stops the run before any fixture
/// runs.
#[cfg(unix)]
fn check_unreadable_file_in_dir(command: &str, extra_args: &[&str], file: &str) {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    fs::copy(fixture(file), dir.path().join(file)).unwrap();
    let unreadable = dir.path().join("unreadable.json");
    fs::copy(fixture(file), &unreadable).unwrap();
    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::File::open(&unreadable).is_ok() {
        // Running as root, which reads the file anyway.
        return
    }

    let output = Command::new(env!("CARGO_BIN_EXE_ef-test-runner"))
        .arg(command)
        .arg(dir.path())
        .args(extra_args)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty(), "{}", String::from_utf8_lossy(&output.stdout));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("cannot read") && stderr.contains("unreadable.json"), "{stderr}");
}

#[cfg(unix)]
#[test]
fn blocktest_exits_2_on_an_unreadable_file_in_a_directory() {
    check_unreadable_file_in_dir("blocktest", &[], "blocktest_empty_block.json");
}

#[cfg(unix)]
#[test]
fn enginetest_exits_2_on_an_unreadable_file_in_a_directory() {
    check_unreadable_file_in_dir("enginetest", &["--workers", "1"], "enginetest_empty_block.json");
}

/// A fixture file that cannot be parsed is reported on stderr and not in the results, the other
/// files still run, and the run exits 2.
fn check_unparsable_file(command: &str, extra_args: &[&str], file: &str) {
    let dir = tempfile::tempdir().unwrap();
    fs::copy(fixture(file), dir.path().join(file)).unwrap();
    let unparsable = dir.path().join("unparsable.json");
    fs::write(&unparsable, "{").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_ef-test-runner"))
        .arg(command)
        .arg(dir.path())
        .args(extra_args)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unparsable.json"), "{stderr}");

    let results: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(results.len(), 1, "{results:?}");
    assert_eq!(results[0]["name"], "empty_block");
    assert_eq!(results[0]["pass"], true);
}

#[test]
fn blocktest_exits_2_on_an_unparsable_file() {
    check_unparsable_file("blocktest", &[], "blocktest_empty_block.json");
}

#[test]
fn enginetest_exits_2_on_an_unparsable_file() {
    check_unparsable_file("enginetest", &["--workers", "1"], "enginetest_empty_block.json");
}

/// With no command and no suite path, the runner prints its usage instead of running.
#[test]
fn no_arguments_print_usage() {
    let output = Command::new(env!("CARGO_BIN_EXE_ef-test-runner")).output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Usage: ef-test-runner"), "{stderr}");
}

/// Many paths run on `--workers` threads at a time, so the open files stay within a low limit.
#[test]
fn blocktest_runs_many_paths_within_the_open_file_limit() {
    const PATHS: usize = 100;
    let dir = tempfile::tempdir().unwrap();
    for i in 0..PATHS {
        fs::copy(fixture("blocktest_empty_block.json"), dir.path().join(format!("{i}.json")))
            .unwrap();
    }

    let output = Command::new("sh")
        .args(["-c", "ulimit -n 128 && exec \"$0\" \"$@\""])
        .arg(env!("CARGO_BIN_EXE_ef-test-runner"))
        .args(["blocktest", "--workers", "2"])
        .arg(dir.path())
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let results: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(results.len(), PATHS);
}
