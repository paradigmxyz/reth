#![allow(missing_docs)]

use std::process::Command;

const RETH: &str = env!("CARGO_BIN_EXE_reth");

#[test]
fn help() {
    let output = Command::new(RETH).arg("--help").output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(stdout.contains("Usage"), "stdout: {stdout}");
    assert!(stdout.contains("node"), "stdout: {stdout}");
}

#[test]
fn version() {
    let output = Command::new(RETH).arg("--version").output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(stdout.to_lowercase().contains("reth"), "stdout: {stdout}");
}

#[test]
fn node_help() {
    let output = Command::new(RETH).args(["node", "--help"]).output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(stdout.contains("--dev"), "stdout: {stdout}");
    assert!(stdout.contains("--http"), "stdout: {stdout}");
}

#[test]
fn unknown_subcommand() {
    let output = Command::new(RETH).arg("definitely-not-a-cmd").output().unwrap();
    assert!(!output.status.success());
}

#[test]
fn unknown_flag() {
    let output = Command::new(RETH).args(["node", "--no-such-flag"]).output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(stderr.contains("--no-such-flag"), "stderr: {stderr}");
}

#[tokio::test]
async fn dev_node_eth_syncing() {
    use alloy_node_bindings::Reth;
    use alloy_provider::{Provider, ProviderBuilder};

    let reth = Reth::at(RETH)
        .dev()
        .disable_discovery()
        .args(["--max-outbound-peers", "0", "--max-inbound-peers", "0"])
        .spawn();

    let provider = ProviderBuilder::new().connect_http(reth.endpoint().parse().unwrap());

    // eth_syncing should not fail on a dev node
    let _syncing = provider.syncing().await.expect("eth_syncing failed");
}

const fn main() {}
