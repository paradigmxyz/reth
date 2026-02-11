#![allow(missing_docs)]

use std::{
    net::TcpListener,
    process::{Command, Stdio},
    time::Duration,
};

const RETH: &str = env!("CARGO_BIN_EXE_reth");

fn unused_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

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

#[test]
#[ignore = "heavy test: starts a full dev node and queries RPC"]
fn dev_node_rpc() {
    let http_port = unused_port();
    let authrpc_port = unused_port();
    let datadir = tempfile::tempdir().unwrap();

    let mut child = Command::new(RETH)
        .args([
            "node",
            "--dev",
            "--http.port",
            &http_port.to_string(),
            "--authrpc.port",
            &authrpc_port.to_string(),
            "--port",
            "0",
            "--ipcdisable",
            "--no-persist-peers",
            "--datadir",
        ])
        .arg(datadir.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let url = format!("http://127.0.0.1:{http_port}");

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let client =
            reqwest::blocking::Client::builder().timeout(Duration::from_secs(5)).build().unwrap();

        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        let mut ready = false;

        while std::time::Instant::now() < deadline {
            let req = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "web3_clientVersion",
                "params": [],
                "id": 1,
            });

            match client.post(&url).json(&req).send() {
                Ok(resp) if resp.status().is_success() => {
                    let body: serde_json::Value = resp.json().unwrap();
                    let version = body["result"].as_str().unwrap();
                    assert!(version.contains("reth"), "unexpected client version: {version}");
                    ready = true;
                    break;
                }
                _ => std::thread::sleep(Duration::from_millis(500)),
            }
        }
        assert!(ready, "node did not become ready within 60s at {url}");

        let resp = client
            .post(&url)
            .json(&serde_json::json!({
                "jsonrpc": "2.0",
                "method": "eth_chainId",
                "params": [],
                "id": 2,
            }))
            .send()
            .unwrap();
        assert!(resp.status().is_success());
        let body: serde_json::Value = resp.json().unwrap();
        let chain_id = body["result"].as_str().unwrap();
        assert!(chain_id.starts_with("0x"), "unexpected chain id: {chain_id}");

        let resp = client
            .post(&url)
            .json(&serde_json::json!({
                "jsonrpc": "2.0",
                "method": "eth_blockNumber",
                "params": [],
                "id": 3,
            }))
            .send()
            .unwrap();
        assert!(resp.status().is_success());
        let body: serde_json::Value = resp.json().unwrap();
        let block_number = body["result"].as_str().unwrap();
        assert!(block_number.starts_with("0x"), "unexpected block number: {block_number}");
    }));

    let _ = child.kill();
    let _ = child.wait();

    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

const fn main() {}
