mod server;

use self::server::SnapshotServer;
use super::{reth_ok, RETH};
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
            entry("txs-0.tar.zst", "static_files/txs-0"),
            entry("txs-1.tar.zst", "static_files/txs-1"),
        ];
        let manifest = json!({
            "block": 19, "chain_id": 1, "storage_version": 2, "timestamp": 0,
            "components": {
                "state": state, "headers": headers,
                "transactions": {
                    "blocks_per_file": 10, "total_blocks": 20,
                    "chunk_files": ["txs-0.tar.zst", "txs-1.tar.zst"],
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
    let snapshot = Snapshot::new();
    let state = &snapshot.files["/snapshot/state.tar.zst"];
    let offset = state.len() / 2;
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join(".download-cache")).unwrap();
    fs::write(dir.path().join(".download-cache/state.tar.zst.part"), &state[..offset]).unwrap();
    let server = snapshot.serve(true).await;
    let args = ["--with-txs-since", "10"];
    success(download(&server, dir.path(), &args));
    for (path, expected) in [
        ("db/snapshot", "state.tar.zst"),
        ("static_files/headers", "headers.tar.zst"),
        ("static_files/txs-1", "txs-1.tar.zst"),
    ] {
        assert_eq!(fs::read(dir.path().join(path)).unwrap(), expected.as_bytes());
    }
    assert!(server.requests().iter().any(|(path, range)| {
        path.ends_with("state.tar.zst") && range.as_deref() == Some(&format!("bytes={offset}-"))
    }));
    assert!(!dir.path().join("static_files/txs-0").exists());
    assert!(!server.requests().iter().any(|(path, _)| path.ends_with("txs-0.tar.zst")));
    let config: toml::Value =
        toml::from_str(&fs::read_to_string(dir.path().join("reth.toml")).unwrap()).unwrap();
    assert_eq!(config["prune"]["segments"]["bodies_history"]["before"].as_integer(), Some(10));

    server.clear_requests();
    success(download(&server, dir.path(), &args));
    assert_eq!(server.requests(), vec![("/snapshot/manifest.json".into(), None)]);

    // Same-size corruption must be detected by checksum, not just the file length.
    fs::write(dir.path().join("static_files/txs-1"), b"bad-1.tar.zst").unwrap();
    server.clear_requests();
    success(download(&server, dir.path(), &args));
    assert_eq!(fs::read(dir.path().join("static_files/txs-1")).unwrap(), b"txs-1.tar.zst");
    assert!(server.requests().iter().any(|(path, _)| path.ends_with("txs-1.tar.zst")));
    assert!(server
        .requests()
        .iter()
        .all(|(path, _)| path.ends_with("manifest.json") || path.ends_with("txs-1.tar.zst")));
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
async fn wrong_chain_is_rejected_before_force_removes_data() {
    let mut snapshot = Snapshot::new();
    snapshot.manifest["chain_id"] = json!(11155111);
    let server = snapshot.serve(true).await;
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("db")).unwrap();
    fs::write(dir.path().join("db/existing"), b"keep").unwrap();
    let output = download(&server, dir.path(), &["--force"]);
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
        download(&server, dir.path(), &["--with-txs-since", "20", "--retry-backoff", "0ms"]);
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
