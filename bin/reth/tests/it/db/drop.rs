use super::*;
use reth_chainspec::{DEV, SEPOLIA};
use reth_provider::providers::RocksDBProvider;

#[cfg(unix)]
use std::os::unix::fs::{symlink, PermissionsExt};

const DROP: &[&str] = &["drop", "--force"];

#[track_caller]
fn run(cmd: &str, dir: &Path, chain: &str, args: &[&str]) -> String {
    reth_ok(&[&[cmd, "--datadir", dir.to_str().unwrap(), "--chain", chain], args].concat())
}

#[track_caller]
fn drop_error(dir: &Path, chain: &str, args: &[&str]) -> String {
    let args = [&["db", "--datadir", dir.to_str().unwrap(), "--chain", chain], args, DROP].concat();
    let output = Command::new(RETH).env("RUST_LOG", "off").args(&args).output().unwrap();
    assert_eq!(output.status.code(), Some(1), "{args:?}: {output:?}");
    String::from_utf8(output.stderr).unwrap().lines().next().unwrap().to_owned()
}

#[test]
fn cleans_custom_rocksdb_history() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("node");
    let rocks = temp.path().join("external");
    let rocks_args = ["--datadir.rocksdb", rocks.to_str().unwrap()];
    let old = Address::ZERO.to_string();
    let old_chain = json!({
        "config": { "chainId": DEV.chain.id() },
        "alloc": { (&old): { "balance": "0x1" } }
    })
    .to_string();
    let new = DEV.genesis.alloc.keys().next().unwrap().to_string();
    let invoke =
        |cmd, chain: &str, args: &[&str]| run(cmd, &dir, chain, &[&rocks_args[..], args].concat());
    let history =
        |chain, addr: &str| invoke("db", chain, &["get", "rocksdb", "accounts-history", addr]);
    invoke("init", &old_chain, &[]);
    let archive = rocks.join("archive");
    fs::create_dir_all(&archive).unwrap();
    let log = archive.join("20261123.log");
    fs::write(&log, b"keep").unwrap();
    assert_eq!(
        drop_error(&dir, &old_chain, &rocks_args),
        format!("Error: Move the contents of {} before dropping RocksDB", archive.display())
    );
    let foreign = temp.path().join("foreign");
    drop(RocksDBProvider::new(&foreign).unwrap());
    for path in [&archive, &foreign] {
        let path = path.to_str().unwrap();
        let error = drop_error(&dir, &old_chain, &["--datadir.rocksdb", path]);
        assert_eq!(
            error,
            format!("Error: Refusing to destroy an unrecognized RocksDB directory at {path}")
        );
    }
    assert!(foreign.join("CURRENT").is_file());
    let keep = rocks.join("keep.txt");
    fs::rename(log, &keep).unwrap();
    assert_eq!(invoke("db", &old_chain, &["drop"]), format!(
        "Permanently delete data at these paths?\n  MDBX: {}\n  RocksDB (dedicated directory): {}\n  Static files: {}\n  ExEx WAL: {}\nThis cannot be undone. (y/N): Database drop aborted!\n",
        dir.join("db").display(), rocks.display(), dir.join("static_files").display(), dir.join("exex/wal").display()
    ));
    assert_eq!(
        drop_error(&dir, "dev", &rocks_args),
        format!("Error: Database does not contain the configured genesis {}", DEV.genesis_hash())
    );
    let writer = RocksDBProvider::new(&rocks).unwrap();
    assert_eq!(
        drop_error(&dir, &old_chain, &rocks_args),
        format!("Error: Could not destroy RocksDB at {}", rocks.display())
    );
    drop(writer);
    assert_eq!(serde_json::from_str::<Vec<u64>>(&history(&old_chain, &old)).unwrap(), [0]);
    invoke("db", &old_chain, DROP);
    invoke("init", "dev", &[]);
    assert_eq!(history("dev", &old), "");
    assert_eq!(serde_json::from_str::<Vec<u64>>(&history("dev", &new)).unwrap(), [0]);
    assert_eq!(fs::read(keep).unwrap(), b"keep");
}

#[test]
fn clears_imported_transaction_lookup() {
    let fixture =
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../crates/rpc/rpc-e2e-tests/testdata/rpc-compat");
    let node = tempfile::tempdir().unwrap();
    let chain = format!("{fixture}/genesis.json");
    let dir = node.path();
    let invoke = |cmd, args: &[&str]| run(cmd, dir, &chain, args);
    let blocks = format!("{fixture}/chain.rlp");
    invoke("import", &["--storage.v2=true", "--fail-on-invalid-block", &blocks]);
    // The third transaction in block 2 has transaction number 6.
    let hash = "0x5bc704d4eb4ce7fe319705d2f888516961426a177f2799c9f934b5df7466dd33";
    let lookup = ["get", "rocksdb", "transaction-hash-numbers", hash];

    let refused =
        |path: &Path| drop_error(dir, &chain, &["--datadir.rocksdb", path.to_str().unwrap()]);
    let root = dir.canonicalize().unwrap();
    assert_eq!(
        refused(dir),
        format!("Error: Database directories overlap: {root:?} and {:?}", root.join("db"))
    );
    #[cfg(unix)]
    {
        let rocks = dir.join("rocksdb");
        let original = rocks.metadata().unwrap().permissions();
        fs::set_permissions(&rocks, fs::Permissions::from_mode(0o300)).unwrap();
        // Privileged runners may still be able to list the directory.
        if let Err(err) = fs::read_dir(&rocks) {
            assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
            assert_eq!(
                drop_error(dir, &chain, &[]),
                format!("Error: failed to read dir {rocks:?}: {err}")
            );
        }
        fs::set_permissions(&rocks, original).unwrap();
        let offline = dir.join("offline");
        symlink(dir.join("unmounted"), &offline).unwrap();
        for path in [&offline, &offline.join("rocksdb")] {
            assert_eq!(refused(path), format!("Error: Could not resolve {}", offline.display()));
        }
    }
    assert_eq!(invoke("db", &lookup).trim(), "6");
    invoke("db", DROP);
    invoke("init", &["--storage.v2=true"]);
    assert_eq!(invoke("db", &lookup), "");
}

#[test]
fn checks_static_genesis_and_drops_legacy_stores() {
    let temp = tempfile::tempdir().unwrap();
    let dev = temp.path().join("dev");
    let sepolia = temp.path().join("sepolia");
    let dev_static = dev.join("static_files");
    run("init", &dev, "dev", &[]);
    run("init", &sepolia, "sepolia", &["--storage.v2=false"]);
    let mismatch = format!(
        "Error: Static files at {} do not contain the configured genesis {}",
        dev_static.display(),
        SEPOLIA.genesis_hash()
    );
    let foreign = ["--datadir.static-files", dev_static.to_str().unwrap()];
    assert_eq!(drop_error(&sepolia, "sepolia", &foreign), mismatch);
    run("db", &dev, "dev", &["clear", "mdbx", "HeaderNumbers"]);
    assert_eq!(drop_error(&dev, "sepolia", &[]), mismatch);
    let missing = dev.join("missing-static");
    assert_eq!(
        drop_error(&dev, "dev", &["--datadir.static-files", missing.to_str().unwrap()]),
        format!("Error: Database does not contain the configured genesis {}", DEV.genesis_hash())
    );
    let rocks = sepolia.join("rocksdb");
    assert!(rocks.join("CURRENT").is_file());
    assert!(dev.join("rocksdb/CURRENT").is_file());
    run("db", &dev, "dev", DROP);

    // Model a legacy V1 datadir that predates RocksDB.
    fs::remove_dir_all(&rocks).unwrap();
    fs::create_dir_all(sepolia.join("exex/wal")).unwrap();
    run("db", &sepolia, "sepolia", DROP);
    assert!(!rocks.exists());
    assert!(!sepolia.join("db").exists());
    assert!(!sepolia.join("exex/wal").exists());
    assert_eq!(fs::read_dir(sepolia.join("static_files")).unwrap().count(), 0);
}
