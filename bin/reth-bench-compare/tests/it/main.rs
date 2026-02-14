#![allow(missing_docs)]

use std::process::Command;

const RETH_BENCH_COMPARE: &str = env!("CARGO_BIN_EXE_reth-bench-compare");

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Runs `reth-bench-compare <args>` and returns stdout, asserting exit code 0.
///
/// Tracing is suppressed via `RUST_LOG=off` so that log lines emitted during
/// binary startup don't pollute stdout-based assertions.
#[track_caller]
fn reth_bench_compare_ok(args: &[&str]) -> String {
    let output =
        Command::new(RETH_BENCH_COMPARE).env("RUST_LOG", "off").args(args).output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "args {args:?} failed.\nstdout: {stdout}\nstderr: {stderr}");
    stdout.into_owned()
}

// ── CLI tests ───────────────────────────────────────────────────────────────

#[test]
fn help() {
    let stdout = reth_bench_compare_ok(&["--help"]);
    assert!(stdout.contains("Usage"), "stdout: {stdout}");
    assert!(stdout.contains("--baseline-ref"), "stdout: {stdout}");
    assert!(stdout.contains("--feature-ref"), "stdout: {stdout}");
}

#[test]
fn version() {
    let stdout = reth_bench_compare_ok(&["--version"]);
    assert!(stdout.contains("reth-bench-compare"), "stdout: {stdout}");
}

#[test]
fn missing_required_args() {
    let output = Command::new(RETH_BENCH_COMPARE).output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(stderr.contains("--baseline-ref"), "stderr: {stderr}");
}

#[test]
fn missing_feature_ref() {
    let output =
        Command::new(RETH_BENCH_COMPARE).args(["--baseline-ref", "main"]).output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(stderr.contains("--feature-ref"), "stderr: {stderr}");
}

const fn main() {}
