mod server;

use self::server::SnapshotServer;
use super::*;
use alloy_provider::{transport::Authorization, ConnectionConfig};
use alloy_rpc_types::engine::{ExecutionPayloadBodyV1, JwtSecret};
use alloy_rpc_types_eth::{error::EthRpcErrorCode, BlockNumberOrTag};
use reth_ethereum_primitives::Block;
use reth_primitives_traits::{Block as _, BlockBody};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    process::{Command, Output},
};

/// Tiny transport fixtures: real tar/zstd archives and checksums, with distinct payloads
/// standing in for database/static files. These test download preparation, not node sync.
struct Snapshot {
    files: BTreeMap<String, Vec<u8>>,
    manifest: Value,
}

impl Snapshot {
    fn new() -> Self {
        let mut files = BTreeMap::new();
        let mut entry = |archive: &str, path: &str| {
            let data = archive.as_bytes();
            let bytes = archive_bytes(path, data);
            let metadata = json!({
                "file": archive, "size": bytes.len(),
                "output_files": [{"path": path, "size": data.len(), "blake3": blake3::hash(data).to_hex().to_string()}]
            });
            files.insert(format!("/snapshot/{archive}"), bytes);
            metadata
        };
        let state = entry("state.tar.zst", "db/snapshot");
        let headers = entry("headers.tar.zst", "static_files/headers");
        let chunks = [
            entry("receipts-0.tar.zst", "static_files/receipts-0"),
            entry("receipts-1.tar.zst", "static_files/receipts-1"),
        ];
        let manifest = json!({
            "block": 19, "chain_id": 1, "storage_version": 2, "timestamp": 0,
            "components": {
                "state": state, "headers": headers,
                "receipts": {
                    "blocks_per_file": 10, "total_blocks": 20,
                    "chunk_files": ["receipts-0.tar.zst", "receipts-1.tar.zst"],
                    "chunk_sizes": [chunks[0]["size"], chunks[1]["size"]],
                    "chunk_output_files": [chunks[0]["output_files"], chunks[1]["output_files"]]
                }
            }
        });
        Self { files, manifest }
    }

    async fn serve(mut self, ranges: bool) -> SnapshotServer {
        self.files
            .insert("/snapshot/manifest.json".into(), serde_json::to_vec(&self.manifest).unwrap());
        SnapshotServer::start(self.files, ranges).await
    }
}

fn archive_bytes(path: &str, data: &[u8]) -> Vec<u8> {
    let mut archive = tar::Builder::new(zstd::Encoder::new(Vec::new(), 0).unwrap());
    let mut header = tar::Header::new_gnu();
    header.set_size(data.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    archive.append_data(&mut header, path, data).unwrap();
    archive.into_inner().unwrap().finish().unwrap()
}

fn download(server: &SnapshotServer, datadir: &Path, args: &[&str]) -> Output {
    Command::new(RETH)
        .env("RUST_LOG", "off")
        .args(["download", "--datadir"])
        .arg(datadir)
        .args([
            "--manifest-url",
            &format!("{}/snapshot/manifest.json", server.url),
            "--non-interactive",
        ])
        .args(args)
        .output()
        .unwrap()
}

#[track_caller]
fn success(output: Output) -> String {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn manifest_selection_reuse_and_repair() {
    check_manifest_selection_reuse_and_repair(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn custom_static_files_selection_reuse_and_repair() {
    check_manifest_selection_reuse_and_repair(true).await;
}

async fn check_manifest_selection_reuse_and_repair(custom_static_files: bool) {
    let snapshot = Snapshot::new();
    let state = &snapshot.files["/snapshot/state.tar.zst"];
    let offset = state.len() / 2;
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join(".download-cache")).unwrap();
    fs::write(dir.path().join(".download-cache/state.tar.zst.part"), &state[..offset]).unwrap();
    let server = snapshot.serve(true).await;
    let custom_dir = tempfile::tempdir().unwrap();
    let static_files = if custom_static_files {
        custom_dir.path().join("nested/static")
    } else {
        dir.path().join("static_files")
    };
    let mut args = vec!["--with-receipts-since", "10"];
    if custom_static_files {
        args.extend(["--datadir.static-files", static_files.to_str().unwrap()]);
    }
    success(download(&server, dir.path(), &args));
    for (path, expected) in [
        (dir.path().join("db/snapshot"), "state.tar.zst"),
        (static_files.join("headers"), "headers.tar.zst"),
        (static_files.join("receipts-1"), "receipts-1.tar.zst"),
    ] {
        assert_eq!(fs::read(path).unwrap(), expected.as_bytes());
    }
    assert!(server.requests().iter().any(|(path, range)| {
        path.ends_with("state.tar.zst") && range.as_deref() == Some(&format!("bytes={offset}-"))
    }));
    assert!(!static_files.join("receipts-0").exists());
    assert!(!server.requests().iter().any(|(path, _)| path.ends_with("receipts-0.tar.zst")));
    let config: toml::Value =
        toml::from_str(&fs::read_to_string(dir.path().join("reth.toml")).unwrap()).unwrap();
    assert_eq!(config["prune"]["segments"]["receipts"]["before"].as_integer(), Some(10));

    server.clear_requests();
    success(download(&server, dir.path(), &args));
    assert_eq!(server.requests(), vec![("/snapshot/manifest.json".into(), None)]);

    // Same-size corruption must be detected by checksum, not just the file length.
    fs::write(static_files.join("receipts-1"), b"Receipts-1.tar.zst").unwrap();
    server.clear_requests();
    success(download(&server, dir.path(), &args));
    assert_eq!(fs::read(static_files.join("receipts-1")).unwrap(), b"receipts-1.tar.zst");
    assert!(server.requests().iter().any(|(path, _)| path.ends_with("receipts-1.tar.zst")));
    assert!(server
        .requests()
        .iter()
        .all(|(path, _)| path.ends_with("manifest.json") || path.ends_with("receipts-1.tar.zst")));
    if custom_static_files {
        assert!(!dir.path().join("static_files").exists());
    }
    fs::write(static_files.join("stale"), b"stale").unwrap();
    args.push("--force");
    success(download(&server, dir.path(), &args));
    assert!(!static_files.join("stale").exists());
    assert_eq!(fs::read(static_files.join("headers")).unwrap(), b"headers.tar.zst");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plan_does_not_download_or_create_datadir() {
    let server = Snapshot::new().serve(true).await;
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("new");
    let output = success(download(&server, &target, &["--archive", "--print-plan-json"]));
    let plan: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(plan["archives"].as_array().unwrap().len(), 4);
    assert!(!target.exists());
    assert_eq!(server.requests(), vec![("/snapshot/manifest.json".into(), None)]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn legacy_streaming_and_resume() {
    for (resumable, ranges) in [(false, false), (true, true), (true, false)] {
        let bytes = archive_bytes("db/snapshot", b"snapshot contents");
        let offset = bytes.len() / 2;
        let server = SnapshotServer::start(
            BTreeMap::from([("/state.tar.zst".into(), bytes.clone())]),
            ranges,
        )
        .await;
        let dir = tempfile::tempdir().unwrap();
        if resumable {
            fs::write(dir.path().join("state.tar.zst.part"), &bytes[..offset]).unwrap();
        }
        reth_ok(&[
            "download",
            "--url",
            &format!("{}/state.tar.zst", server.url),
            "--datadir",
            dir.path().to_str().unwrap(),
            if resumable { "--resumable=true" } else { "--resumable=false" },
        ]);
        assert_eq!(fs::read(dir.path().join("db/snapshot")).unwrap(), b"snapshot contents");
        assert!(!dir.path().join("state.tar.zst.part").exists());
        assert!(!dir.path().join("state.tar.zst").exists());
        if resumable {
            assert!(server
                .requests()
                .iter()
                .any(|(_, range)| range.as_deref() == Some(&format!("bytes={offset}-"))));
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn custom_static_files_legacy_http_and_local_archives() {
    for local in [false, true] {
        let bytes = archive_bytes("./static_files/nested/headers", b"snapshot contents");
        let archive_name = "headers.tar.zst";
        let source = tempfile::tempdir().unwrap();
        let archive_path = source.path().join(archive_name);
        fs::write(&archive_path, &bytes).unwrap();
        let server =
            SnapshotServer::start(BTreeMap::from([(format!("/{archive_name}"), bytes)]), true)
                .await;
        let url = if local {
            format!("file://{}", archive_path.display())
        } else {
            format!("{}/{archive_name}", server.url)
        };
        let dir = tempfile::tempdir().unwrap();
        let static_dir = tempfile::tempdir().unwrap();
        reth_ok(&[
            "download",
            "--url",
            &url,
            "--datadir",
            dir.path().to_str().unwrap(),
            "--datadir.static-files",
            static_dir.path().to_str().unwrap(),
        ]);
        assert_eq!(
            fs::read(static_dir.path().join("nested/headers")).unwrap(),
            b"snapshot contents"
        );
        assert!(!dir.path().join("static_files").exists());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wrong_chain_is_rejected_before_force_removes_data() {
    let mut snapshot = Snapshot::new();
    snapshot.manifest["chain_id"] = json!(11155111);
    let server = snapshot.serve(true).await;
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("db")).unwrap();
    fs::write(dir.path().join("db/existing"), b"keep").unwrap();
    let output = download(&server, dir.path(), &["--full", "--force"]);
    assert!(!output.status.success(), "a Sepolia snapshot must not be installed for mainnet");
    assert!(String::from_utf8_lossy(&output.stderr).contains("chain ID"));
    assert_eq!(fs::read(dir.path().join("db/existing")).unwrap(), b"keep");
    assert_eq!(server.requests(), vec![("/snapshot/manifest.json".into(), None)]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bad_output_checksum_prevents_finalization() {
    let mut snapshot = Snapshot::new();
    snapshot.manifest["components"]["state"]["output_files"][0]["blake3"] =
        json!(blake3::hash(b"wrong").to_hex().to_string());
    let server = snapshot.serve(true).await;
    let dir = tempfile::tempdir().unwrap();
    let output =
        download(&server, dir.path(), &["--with-receipts-since", "20", "--retry-backoff", "0ms"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Failed integrity validation"));
    assert!(String::from_utf8_lossy(&output.stderr).contains("after 10 attempts"));
    assert_eq!(
        server
            .requests()
            .iter()
            .filter(|(path, range)| { path == "/snapshot/state.tar.zst" && range.is_none() })
            .count(),
        10,
        "each checksum failure must fetch a fresh archive"
    );
    assert!(!dir.path().join("reth.toml").exists());
    assert!(!dir.path().join("db/mdbx.dat").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn completion_only_suggests_startup_for_matching_chain() {
    for (chain_id, expected_suffix) in [(1, " Run `reth node` to start syncing."), (42431, "")] {
        let mut snapshot = Snapshot::new();
        snapshot.manifest["chain_id"] = json!(chain_id);
        let server = snapshot.serve(true).await;
        let dir = tempfile::tempdir().unwrap();
        let output = Command::new(RETH)
            .env("RUST_LOG", "reth::cli=info")
            .args(["download", "--color", "never", "--archive", "--non-interactive"])
            .args(["--manifest-url", &format!("{}/snapshot/manifest.json", server.url)])
            .arg("--datadir")
            .arg(dir.path())
            .output()
            .unwrap();
        let logs = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        success(output);
        let suffix = logs
            .lines()
            .find_map(|line| {
                line.split_once("Snapshot download complete.").map(|(_, suffix)| suffix)
            })
            .unwrap();
        assert_eq!(suffix, expected_suffix);
        assert_eq!(fs::read(dir.path().join("db/snapshot")).unwrap(), b"state.tar.zst");
        assert_eq!(
            fs::read(dir.path().join("static_files/receipts-0")).unwrap(),
            b"receipts-0.tar.zst"
        );
        assert!(dir.path().join("reth.toml").exists());
    }
}

#[tokio::test]
async fn future_transaction_cutoff_restores_a_bootable_node() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../crates/rpc/rpc-e2e-tests/testdata/rpc-compat");
    let chain = fixture.join("genesis.json");
    let chain = chain.to_str().unwrap();
    let source = tempfile::tempdir().unwrap();
    let archives = tempfile::tempdir().unwrap();
    let blocks = fs::read(fixture.join("chain.rlp")).unwrap();
    let mut next_block = blocks.as_slice();
    for _ in 0..44 {
        Block::decode_sealed(&mut next_block).unwrap();
    }
    let prefix = archives.path().join("prefix.rlp");
    let suffix = archives.path().join("next.rlp");
    fs::write(&prefix, &blocks[..blocks.len() - next_block.len()]).unwrap();
    fs::write(&suffix, next_block).unwrap();
    let expected = Block::decode_sealed(&mut next_block).unwrap();
    assert!(next_block.is_empty());
    assert_eq!(expected.body().transactions.len(), 6);
    let run = |name: &str, datadir: &Path, args: &[&str]| {
        let mut command = Command::new(RETH);
        command.args([name, "--chain", chain, "--datadir"]).arg(datadir);
        command.args(["--db.max-size=256MB", "--db.growth-step=8MB"]);
        success(command.args(args).output().unwrap());
    };
    run("import", source.path(), &[prefix.to_str().unwrap()]);
    // Pre-Cancun slot preimages are auxiliary to this restore and occupy a sparse 4 GiB file.
    fs::remove_dir_all(source.path().join("db/preimage")).unwrap();
    reth_ok(&[
        "snapshot-manifest",
        "-d",
        source.path().to_str().unwrap(),
        "-o",
        archives.path().to_str().unwrap(),
        "--chain-id=3503995874084926",
    ]);
    let manifest = archives.path().join("manifest.json");
    let snapshot: Value = serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
    let tip = snapshot["block"].as_u64().unwrap();
    assert_eq!(tip, 44);
    assert!(snapshot["components"]["rocksdb_indices"].is_object());

    for include_indices in [false, true] {
        let target = tempfile::tempdir().unwrap();
        let mut flags = vec![
            "--manifest-path",
            manifest.to_str().unwrap(),
            "--with-txs-since=100",
            "--with-state-history",
            "--with-receipts",
            "--with-senders",
        ];
        if include_indices {
            flags.push("--with-rocksdb");
        }
        run("download", target.path(), &flags);
        let config: toml::Value =
            toml::from_str(&fs::read_to_string(target.path().join("reth.toml")).unwrap()).unwrap();
        assert_eq!(config["prune"]["segments"]["bodies_history"]["before"].as_integer(), Some(100));
        let start_node = || {
            let node = Reth::at(RETH)
                .instance(0)
                .chain_or_path(chain)
                .data_dir(target.path())
                .disable_discovery()
                .args(["--with-unused-ports", "--max-outbound-peers=0", "--max-inbound-peers=0"])
                .spawn();
            let rpc = ProviderBuilder::new().connect_http(node.endpoint_url());
            (node, rpc)
        };
        let (node, rpc) = start_node();
        assert_eq!(rpc.get_block_number().await.unwrap(), tip);
        let capabilities: Value = rpc.client().request_noparams("eth_capabilities").await.unwrap();
        assert_eq!(capabilities["blocks"]["oldestBlock"], format!("{:#x}", tip + 1));
        let err = rpc.get_block_by_number(tip.into()).await.unwrap_err();
        let error = err.as_error_resp().unwrap();
        assert_eq!(error.code, i64::from(EthRpcErrorCode::PrunedHistory.code()));
        assert_eq!(
            error.message,
            "pruned history unavailable: requested 44, earliest available 45"
        );
        drop(node);

        run("import", target.path(), &[suffix.to_str().unwrap()]);
        let (node, rpc) = start_node();
        assert_eq!(rpc.get_block_number().await.unwrap(), tip + 1);
        let block = rpc.get_block_by_number(BlockNumberOrTag::Earliest).await.unwrap().unwrap();
        assert_eq!(block.header.hash, expected.hash());
        assert!(block.transactions.hashes().eq(expected.body().transaction_hashes_iter().copied()));

        let secret = JwtSecret::from_file(&target.path().join("jwt.hex")).unwrap();
        let token = secret.encode(&Default::default()).unwrap();
        let engine = ProviderBuilder::new()
            .connect_with_config(
                &format!("http://{}:{}", node.host(), node.auth_port().unwrap()),
                ConnectionConfig::default().with_auth(Authorization::bearer(token)),
            )
            .await
            .unwrap();
        let hashes = [expected.parent_hash, expected.hash()];
        let by_hash: Vec<Option<ExecutionPayloadBodyV1>> =
            engine.client().request("engine_getPayloadBodiesByHashV1", (hashes,)).await.unwrap();
        let by_range: Vec<Option<ExecutionPayloadBodyV1>> = engine
            .client()
            .request("engine_getPayloadBodiesByRangeV1", (format!("{tip:#x}"), "0x2"))
            .await
            .unwrap();
        let body = expected.body();
        let payload = ExecutionPayloadBodyV1::new(body.withdrawals.clone(), &body.transactions);
        assert_eq!(by_hash, [None, Some(payload)]);
        assert_eq!(by_range, by_hash);
    }
}
