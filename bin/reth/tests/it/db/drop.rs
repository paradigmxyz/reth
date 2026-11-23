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
    // The old and new genesis must differ even though their chain IDs match.
    let mut genesis = SEPOLIA.genesis.clone();
    genesis.config.chain_id = DEV.chain.id();
    let old = genesis.alloc.keys().next().unwrap().to_string();
    let new = DEV.genesis.alloc.keys().next().unwrap().to_string();
    let old_chain = serde_json::to_string(&genesis).unwrap();
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
    }
    assert_eq!(invoke("db", &lookup).trim(), "6");
    invoke("db", DROP);
    invoke("init", &["--storage.v2=true"]);
    assert_eq!(invoke("db", &lookup), "");
}

#[test]
fn protects_external_directories_and_drops_legacy_stores() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("node");
    let init_rocks = temp.path().join("init-rocksdb");
    let v1 = ["--storage.v2=false", "--datadir.rocksdb", init_rocks.to_str().unwrap()];
    run("init", &dir, "dev", &v1);
    let external = temp.path().join("external");
    fs::create_dir(&external).unwrap();
    let log = external.join("000007.log");
    fs::write(&log, b"keep").unwrap();
    let refused =
        |path: &Path| drop_error(&dir, "dev", &["--datadir.rocksdb", path.to_str().unwrap()]);
    let unrecognized = format!(
        "Error: Refusing to destroy an unrecognized RocksDB directory at {}",
        external.display()
    );
    assert_eq!(refused(&external), unrecognized);
    assert_eq!(fs::read(&log).unwrap(), b"keep");
    fs::remove_file(log).unwrap();
    drop(RocksDBProvider::new(&external).unwrap());
    assert_eq!(refused(&external), unrecognized);
    assert!(external.join("CURRENT").is_file());
    #[cfg(unix)]
    {
        let offline = dir.join("offline");
        symlink(dir.join("unmounted"), &offline).unwrap();
        for path in [&offline, &offline.join("rocksdb")] {
            assert_eq!(refused(path), format!("Error: Could not resolve {}", offline.display()));
        }
        assert!(dir.join("db/mdbx.dat").is_file());
    }
    let root = dir.canonicalize().unwrap();
    assert_eq!(
        refused(&dir),
        format!("Error: Database directories overlap: {root:?} and {:?}", root.join("db"))
    );
    fs::create_dir_all(dir.join("exex/wal")).unwrap();
    run("db", &dir, "dev", DROP);
    assert!(!dir.join("rocksdb").exists());
    assert!(!dir.join("db").exists());
    assert!(!dir.join("exex/wal").exists());
    assert_eq!(fs::read_dir(dir.join("static_files")).unwrap().count(), 0);
}

#[test]
fn checks_static_genesis_independently() {
    let temp = tempfile::tempdir().unwrap();
    let dev = temp.path().join("dev");
    let sepolia = temp.path().join("sepolia");
    let dev_static = dev.join("static_files");
    run("init", &dev, "dev", &[]);
    run("init", &sepolia, "sepolia", &[]);
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
    assert!(sepolia.join("rocksdb/CURRENT").is_file());
    assert!(dev.join("rocksdb/CURRENT").is_file());
    run("db", &dev, "dev", DROP);
}
