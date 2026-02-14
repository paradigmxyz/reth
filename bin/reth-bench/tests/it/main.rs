#![allow(missing_docs)]

use std::process::Command;

const RETH_BENCH: &str = env!("CARGO_BIN_EXE_reth-bench");

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Runs `reth-bench <args>` and returns stdout, asserting exit code 0.
///
/// Tracing is suppressed via `RUST_LOG=off` so that log lines emitted during
/// binary startup don't pollute stdout-based assertions.
#[track_caller]
fn reth_bench_ok(args: &[&str]) -> String {
    let output = Command::new(RETH_BENCH).env("RUST_LOG", "off").args(args).output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "args {args:?} failed.\nstdout: {stdout}\nstderr: {stderr}");
    stdout.into_owned()
}

// ── Top-level CLI tests ─────────────────────────────────────────────────────

#[test]
fn help() {
    let stdout = reth_bench_ok(&["--help"]);
    assert!(stdout.contains("Usage"), "stdout: {stdout}");
    assert!(stdout.contains("new-payload-fcu"), "stdout: {stdout}");
}

#[test]
fn unknown_subcommand() {
    let output = Command::new(RETH_BENCH).arg("definitely-not-a-cmd").output().unwrap();
    assert!(!output.status.success());
}

#[test]
fn unknown_flag() {
    let output =
        Command::new(RETH_BENCH).args(["new-payload-fcu", "--no-such-flag"]).output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(stderr.contains("--no-such-flag"), "stderr: {stderr}");
}

// ── Subcommand --help coverage ──────────────────────────────────────────────
//
// Every registered subcommand must produce valid --help output.  This catches
// clap wiring regressions (e.g. a missing field, a conflicting arg name, or a
// broken `help_message()` call) that would otherwise only surface when a user
// runs the command.

#[test]
fn new_payload_fcu_help() {
    let stdout = reth_bench_ok(&["new-payload-fcu", "--help"]);
    assert!(stdout.contains("--rpc-url"), "stdout: {stdout}");
}

#[test]
fn gas_limit_ramp_help() {
    let stdout = reth_bench_ok(&["gas-limit-ramp", "--help"]);
    assert!(stdout.contains("--engine-rpc-url"), "stdout: {stdout}");
    assert!(stdout.contains("--jwt-secret"), "stdout: {stdout}");
}

#[test]
fn new_payload_only_help() {
    let stdout = reth_bench_ok(&["new-payload-only", "--help"]);
    assert!(stdout.contains("--rpc-url"), "stdout: {stdout}");
}

#[test]
fn send_payload_help() {
    let stdout = reth_bench_ok(&["send-payload", "--help"]);
    assert!(stdout.contains("--mode"), "stdout: {stdout}");
}

#[test]
fn generate_big_block_help() {
    let stdout = reth_bench_ok(&["generate-big-block", "--help"]);
    assert!(stdout.contains("--target-gas"), "stdout: {stdout}");
    assert!(stdout.contains("--engine-rpc-url"), "stdout: {stdout}");
}

#[test]
fn replay_payloads_help() {
    let stdout = reth_bench_ok(&["replay-payloads", "--help"]);
    assert!(stdout.contains("--payload-dir"), "stdout: {stdout}");
    assert!(stdout.contains("--engine-rpc-url"), "stdout: {stdout}");
}

#[test]
fn send_invalid_payload_help() {
    let stdout = reth_bench_ok(&["send-invalid-payload", "--help"]);
    assert!(stdout.contains("--invalidate-state-root"), "stdout: {stdout}");
    assert!(stdout.contains("--dry-run"), "stdout: {stdout}");
}

const fn main() {}
