#![allow(missing_docs)]

use std::{fs, path::Path, process::Command};

const RETH: &str = env!("CARGO_BIN_EXE_reth");

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Runs `reth <args>` and returns stdout, asserting exit code 0.
///
/// Tracing is suppressed via `RUST_LOG=off` so that log lines emitted during
/// binary startup don't pollute stdout-based assertions.
#[track_caller]
fn reth_ok(args: &[&str]) -> String {
    let output = Command::new(RETH).env("RUST_LOG", "off").args(args).output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "args {args:?} failed.\nstdout: {stdout}\nstderr: {stderr}");
    stdout.into_owned()
}

/// Spawns an isolated dev-mode reth node.
///
/// Discovery is disabled and peer limits are zeroed so the node is fully
/// isolated.  Each call gets a unique temporary data directory so that
/// concurrent test runs never collide on the default `reth/dev/` path.
fn spawn_dev() -> (alloy_node_bindings::RethInstance, tempfile::TempDir) {
    use alloy_node_bindings::Reth;

    let datadir = tempfile::tempdir().expect("failed to create temp dir");

    let instance = Reth::at(RETH)
        .dev()
        .disable_discovery()
        .data_dir(datadir.path())
        .args(["--max-outbound-peers", "0", "--max-inbound-peers", "0"])
        .spawn();

    // Return the TempDir alongside the instance so it lives as long as the node.
    (instance, datadir)
}

fn create_tar_zst_snapshot(path: &Path) {
    let file = fs::File::create(path).expect("failed to create snapshot archive");
    let encoder = zstd::Encoder::new(file, 0).expect("failed to create zstd encoder");
    let mut archive = tar::Builder::new(encoder);

    let contents = b"new snapshot data";
    let mut header = tar::Header::new_gnu();
    header.set_path("db/new.snapshot").expect("failed to set archive path");
    header.set_size(contents.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    archive.append(&header, contents.as_slice()).expect("failed to append archive file");

    let encoder = archive.into_inner().expect("failed to finish tar archive");
    encoder.finish().expect("failed to finish zstd archive");
}

// ── Original tests (from PR #22069) ──────────────────────────────────────────

#[test]
fn help() {
    let stdout = reth_ok(&["--help"]);
    assert!(stdout.contains("Usage"), "stdout: {stdout}");
    assert!(stdout.contains("node"), "stdout: {stdout}");
}

#[test]
fn version() {
    let stdout = reth_ok(&["--version"]);
    assert!(stdout.to_lowercase().contains("reth"), "stdout: {stdout}");
}

#[test]
fn node_help() {
    let stdout = reth_ok(&["node", "--help"]);
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
    use alloy_provider::{Provider, ProviderBuilder};

    let (reth, _datadir) = spawn_dev();
    let provider = ProviderBuilder::new().connect_http(reth.endpoint().parse().unwrap());

    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    let _syncing = provider.syncing().await.expect("eth_syncing failed");
}

// ── Subcommand --help coverage ───────────────────────────────────────────────
//
// Every registered subcommand must produce valid --help output.  This catches
// clap wiring regressions (e.g. a missing field, a conflicting arg name, or a
// broken `help_message()` call) that would otherwise only surface when a user
// runs the command.

#[test]
fn init_help() {
    let stdout = reth_ok(&["init", "--help"]);
    assert!(stdout.contains("--chain"), "stdout: {stdout}");
}

#[test]
fn init_state_help() {
    let stdout = reth_ok(&["init-state", "--help"]);
    assert!(stdout.contains("--chain"), "stdout: {stdout}");
}

#[test]
fn import_help() {
    let stdout = reth_ok(&["import", "--help"]);
    assert!(stdout.contains("--chain"), "stdout: {stdout}");
}

#[test]
fn import_era_help() {
    let stdout = reth_ok(&["import-era", "--help"]);
    assert!(stdout.contains("--chain"), "stdout: {stdout}");
}

#[test]
fn export_era_help() {
    let stdout = reth_ok(&["export-era", "--help"]);
    assert!(stdout.contains("--chain"), "stdout: {stdout}");
}

#[test]
fn dump_genesis_help() {
    let stdout = reth_ok(&["dump-genesis", "--help"]);
    assert!(stdout.contains("--chain"), "stdout: {stdout}");
}

#[test]
fn db_help() {
    let stdout = reth_ok(&["db", "--help"]);
    assert!(stdout.contains("stats"), "stdout: {stdout}");
}

#[test]
fn stage_help() {
    let stdout = reth_ok(&["stage", "--help"]);
    assert!(stdout.contains("run"), "stdout: {stdout}");
}

#[test]
fn p2p_help() {
    let stdout = reth_ok(&["p2p", "--help"]);
    assert!(stdout.contains("header"), "stdout: {stdout}");
}

#[test]
fn config_help() {
    let stdout = reth_ok(&["config", "--help"]);
    assert!(stdout.contains("--default"), "stdout: {stdout}");
}

#[test]
fn prune_help() {
    let stdout = reth_ok(&["prune", "--help"]);
    assert!(stdout.contains("--chain"), "stdout: {stdout}");
}

#[test]
fn download_help() {
    let stdout = reth_ok(&["download", "--help"]);
    assert!(stdout.contains("--chain"), "stdout: {stdout}");
    assert!(stdout.contains("--force"), "stdout: {stdout}");
}

#[test]
fn download_force_overwrites_snapshot_data_but_preserves_identity_files() {
    let tempdir = tempfile::tempdir().expect("failed to create temp dir");
    let datadir = tempdir.path().join("datadir");
    let archive_path = tempdir.path().join("snapshot.tar.zst");

    fs::create_dir_all(datadir.join("db")).expect("failed to create old db dir");
    fs::create_dir_all(datadir.join("static_files")).expect("failed to create old static dir");
    fs::write(datadir.join("db/old.mdbx"), b"old db").expect("failed to write old db file");
    fs::write(datadir.join("static_files/old"), b"old static")
        .expect("failed to write old static file");
    fs::write(datadir.join("discovery-secret"), b"secret").expect("failed to write secret");
    fs::write(datadir.join("known-peers.json"), br#"["peer"]"#)
        .expect("failed to write known peers");
    create_tar_zst_snapshot(&archive_path);

    let datadir_arg = datadir.to_str().expect("datadir should be utf8");
    let archive_url = format!("file://{}", archive_path.display());
    reth_ok(&["download", "--datadir", datadir_arg, "--url", &archive_url, "--force"]);

    assert_eq!(
        fs::read(datadir.join("db/new.snapshot")).expect("missing extracted snapshot file"),
        b"new snapshot data"
    );
    assert!(!datadir.join("db/old.mdbx").exists(), "old db file should be removed");
    assert!(!datadir.join("static_files/old").exists(), "old static file should be removed");
    assert_eq!(
        fs::read(datadir.join("discovery-secret")).expect("missing preserved discovery secret"),
        b"secret"
    );
    assert_eq!(
        fs::read(datadir.join("known-peers.json")).expect("missing preserved known peers"),
        br#"["peer"]"#
    );
}

#[test]
fn re_execute_help() {
    let stdout = reth_ok(&["re-execute", "--help"]);
    assert!(stdout.contains("--chain"), "stdout: {stdout}");
}

// ── `config --default` outputs valid TOML ────────────────────────────────────

#[test]
fn config_default_valid_toml() {
    let stdout = reth_ok(&["config", "--default"]);

    let parsed: toml::Value =
        toml::from_str(&stdout).expect("config --default did not produce valid TOML");

    // The default config must contain the [stages] table — this is the heart of
    // the pipeline configuration and its absence would indicate a serialization
    // regression.
    assert!(parsed.get("stages").is_some(), "missing [stages] in config output");
}

// ── `dump-genesis` outputs valid JSON ────────────────────────────────────────

#[test]
fn dump_genesis_mainnet_valid_json() {
    let stdout = reth_ok(&["dump-genesis"]);

    let genesis: serde_json::Value =
        serde_json::from_str(&stdout).expect("dump-genesis did not produce valid JSON");

    assert!(genesis.get("nonce").is_some(), "missing nonce in genesis JSON");
    assert!(genesis.get("alloc").is_some(), "missing alloc in genesis JSON");
}

#[test]
fn dump_genesis_sepolia_valid_json() {
    let stdout = reth_ok(&["dump-genesis", "--chain", "sepolia"]);

    let genesis: serde_json::Value = serde_json::from_str(&stdout)
        .expect("dump-genesis --chain sepolia did not produce valid JSON");

    assert!(genesis.get("alloc").is_some(), "missing alloc in sepolia genesis JSON");
}

// ── Dev node: send transaction round-trip ────────────────────────────────────
//
// Exercises the full pipeline: RPC submission → mempool → sealing → execution →
// receipt retrieval.  Uses the pre-funded dev account so no genesis customization
// is required.

#[tokio::test]
async fn dev_node_send_tx_and_mine() {
    use alloy_primitives::{Address, U256};
    use alloy_provider::{Provider, ProviderBuilder};
    use alloy_rpc_types_eth::TransactionRequest;

    let (reth, _datadir) = spawn_dev();
    let provider = ProviderBuilder::new().connect_http(reth.endpoint().parse().unwrap());

    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    // Dev mode pre-funds the first dev account.
    let accounts = provider.get_accounts().await.expect("eth_accounts failed");
    assert!(!accounts.is_empty(), "dev node should expose at least one account");

    let sender = accounts[0];
    let recipient = Address::with_last_byte(0x42);

    let tx = TransactionRequest::default().from(sender).to(recipient).value(U256::from(1_000_000));

    let tx_hash = provider.send_transaction(tx).await.expect("eth_sendTransaction failed");

    // In dev/instant-mine mode the node seals a block for each transaction, so
    // the receipt becomes available almost immediately.
    let receipt = tx_hash.get_receipt().await.expect("failed to get receipt");

    assert!(receipt.status(), "transaction should have succeeded");
    assert_eq!(receipt.to, Some(recipient));
    assert!(receipt.block_number.unwrap() > 0, "receipt should be in a mined block");

    // Verify the transfer actually mutated state.
    let balance = provider.get_balance(recipient).await.expect("eth_getBalance failed");
    assert_eq!(balance, U256::from(1_000_000));
}

const fn main() {}
