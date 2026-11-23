use super::*;
use reth_chainspec::{ChainSpec, DEV};
use reth_provider::providers::RocksDBProvider;

#[cfg(unix)]
use std::os::unix::fs::{symlink, PermissionsExt};

#[track_caller]
fn reth_error(args: &[&str]) -> String {
    let output = Command::new(RETH).env("RUST_LOG", "off").args(args).output().unwrap();
    assert_eq!(output.status.code(), Some(1), "args {args:?}: {output:?}");
    String::from_utf8(output.stderr).unwrap().lines().next().unwrap().to_owned()
}

#[track_caller]
fn run(check: fn(&[&str]) -> String, cmd: &str, dir: &Path, chain: &str, args: &[&str]) -> String {
    check(&[&[cmd, "--datadir", dir.to_str().unwrap(), "--chain", chain], args].concat())
}

#[test]
fn cleans_custom_rocksdb_history() {
    let node = tempfile::tempdir().unwrap();
    let external = tempfile::tempdir().unwrap();
    let rocks = external.path();
    let genesis = |addr: &str| {
        json!({
            "config": {"chainId": 65500}, "difficulty": "0x1", "gasLimit": "0x1c9c380",
            "alloc": { (addr): { "balance": "0x1" } }
        })
        .to_string()
    };
    let old = Address::with_last_byte(1).to_string();
    let new = Address::with_last_byte(2).to_string();
    let invoke = |check, cmd, chain: &str, args: &[&str]| {
        run(
            check,
            cmd,
            node.path(),
            chain,
            &[&["--datadir.rocksdb", rocks.to_str().unwrap()], args].concat(),
        )
    };
    let history = |chain: &str, addr: &str| {
        invoke(reth_ok, "db", chain, &["get", "rocksdb", "accounts-history", addr])
    };
    let old_chain = genesis(&old);
    let new_chain = genesis(&new);
    invoke(reth_ok, "init", &old_chain, &[]);
    assert_eq!(serde_json::from_str::<Vec<u64>>(&history(&old_chain, &old)).unwrap(), [0]);
    let keep = rocks.join("keep.txt");
    fs::write(&keep, b"keep").unwrap();
    let dir = node.path();
    assert_eq!(invoke(reth_ok, "db", &old_chain, &["drop"]), format!(
        "Permanently delete data at these paths?\n  MDBX: {}\n  RocksDB (dedicated directory): {}\n  Static files: {}\n  ExEx WAL: {}\nThis cannot be undone. (y/N): Database drop aborted!\n",
        dir.join("db").display(), rocks.display(), dir.join("static_files").display(), dir.join("exex/wal").display()
    ));
    let drop_args = ["drop", "--force"];
    let spec = ChainSpec::from_genesis(serde_json::from_str(&new_chain).unwrap());
    assert_eq!(
        invoke(reth_error, "db", &new_chain, &drop_args),
        format!("Error: Database does not contain the configured genesis {}", spec.genesis_hash())
    );
    let archive = rocks.join("archive");
    fs::create_dir_all(&archive).unwrap();
    let log = archive.join("20261123.log");
    fs::write(&log, b"application log").unwrap();
    assert_eq!(
        invoke(reth_error, "db", &old_chain, &drop_args),
        format!("Error: Move the contents of {} before dropping RocksDB", archive.display())
    );
    assert_eq!(fs::read(&log).unwrap(), b"application log");
    fs::remove_file(log).unwrap();
    let writer = RocksDBProvider::new(rocks).unwrap();
    assert_eq!(
        invoke(reth_error, "db", &old_chain, &drop_args),
        format!("Error: Could not destroy RocksDB at {}", rocks.display())
    );
    drop(writer);
    assert_eq!(serde_json::from_str::<Vec<u64>>(&history(&old_chain, &old)).unwrap(), [0]);
    invoke(reth_ok, "db", &old_chain, &drop_args);
    invoke(reth_ok, "init", &new_chain, &[]);
    assert_eq!(history(&new_chain, &old), "");
    assert_eq!(serde_json::from_str::<Vec<u64>>(&history(&new_chain, &new)).unwrap(), [0]);
    assert_eq!(fs::read(keep).unwrap(), b"keep");
}

#[test]
fn clears_imported_transaction_lookup() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../crates/rpc/rpc-e2e-tests/testdata/rpc-compat");
    let node = tempfile::tempdir().unwrap();
    let genesis = fixture.join("genesis.json");
    let invoke =
        |cmd, args: &[&str]| run(reth_ok, cmd, node.path(), genesis.to_str().unwrap(), args);
    invoke(
        "import",
        &[
            "--storage.v2=true",
            "--fail-on-invalid-block",
            fixture.join("chain.rlp").to_str().unwrap(),
        ],
    );
    // The third transaction in block 2 has transaction number 6.
    let lookup = [
        "get",
        "rocksdb",
        "transaction-hash-numbers",
        "0x5bc704d4eb4ce7fe319705d2f888516961426a177f2799c9f934b5df7466dd33",
    ];
    assert_eq!(invoke("db", &lookup).trim(), "6");
    invoke("db", &["drop", "--force"]);
    invoke("init", &["--storage.v2=true"]);
    assert_eq!(invoke("db", &lookup), "");
}

#[test]
fn protects_external_directories_and_drops_legacy_stores() {
    let node = tempfile::tempdir().unwrap();
    let dir = node.path();
    run(reth_ok, "init", dir, "dev", &["--storage.v2=false"]);
    let rocks = dir.join("rocksdb");
    if rocks.exists() {
        fs::remove_dir_all(&rocks).unwrap();
    }
    let external = tempfile::tempdir().unwrap();
    let log = external.path().join("000007.log");
    fs::write(&log, b"keep").unwrap();
    let refused = |path: &Path| {
        run(
            reth_error,
            "db",
            dir,
            "dev",
            &["--datadir.rocksdb", path.to_str().unwrap(), "drop", "--force"],
        )
    };
    assert_eq!(
        refused(external.path()),
        format!(
            "Error: Refusing to destroy an unrecognized RocksDB directory at {}",
            external.path().display()
        )
    );
    assert_eq!(fs::read(log).unwrap(), b"keep");
    let foreign = tempfile::tempdir().unwrap();
    drop(RocksDBProvider::builder(foreign.path()).build().unwrap());
    assert_eq!(
        refused(foreign.path()),
        format!(
            "Error: Refusing to destroy an unrecognized RocksDB directory at {}",
            foreign.path().display()
        )
    );
    assert!(foreign.path().join("CURRENT").is_file());
    #[cfg(unix)]
    {
        let offline = dir.join("offline");
        symlink(dir.join("unmounted"), &offline).unwrap();
        assert_eq!(refused(&offline), format!("Error: Could not resolve {}", offline.display()));
        assert!(dir.join("db/mdbx.dat").is_file());
    }
    let root = dir.canonicalize().unwrap();
    assert_eq!(
        refused(dir),
        format!("Error: Database directories overlap: {root:?} and {:?}", root.join("db"))
    );
    fs::create_dir_all(dir.join("exex/wal")).unwrap();
    run(reth_ok, "db", dir, "dev", &["drop", "--force"]);
    assert!(!rocks.exists());
    assert!(!dir.join("db").exists());
    assert!(!dir.join("exex/wal").exists());
    assert_eq!(fs::read_dir(dir.join("static_files")).unwrap().count(), 0);
}

#[test]
fn checks_static_genesis_independently() {
    let node = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let dir = node.path();
    let foreign = other.path().join("static_files");
    let chain =
        r#"{"config":{"chainId":65500},"difficulty":"0x1","gasLimit":"0x1c9c380","alloc":{}}"#;
    run(reth_ok, "init", dir, "dev", &[]);
    run(reth_ok, "init", other.path(), chain, &[]);
    let mismatch = |path: &Path, hash| {
        format!(
            "Error: Static files at {} do not contain the configured genesis {hash}",
            path.display()
        )
    };
    assert_eq!(
        run(
            reth_error,
            "db",
            dir,
            "dev",
            &["--datadir.static-files", foreign.to_str().unwrap(), "drop", "--force"]
        ),
        mismatch(&foreign, DEV.genesis_hash())
    );
    run(reth_ok, "db", dir, "dev", &["clear", "mdbx", "HeaderNumbers"]);
    let other_hash = ChainSpec::from_genesis(serde_json::from_str(chain).unwrap()).genesis_hash();
    let own = dir.join("static_files");
    assert_eq!(run(reth_error, "db", dir, chain, &["drop", "--force"]), mismatch(&own, other_hash));
    let saved = dir.join("saved");
    fs::rename(&own, &saved).unwrap();
    assert_eq!(
        run(reth_error, "db", dir, "dev", &["drop", "--force"]),
        format!("Error: Database does not contain the configured genesis {}", DEV.genesis_hash())
    );
    if own.exists() {
        fs::remove_dir(&own).unwrap();
    }
    fs::rename(saved, own).unwrap();
    for (path, spec) in [(dir, "dev"), (other.path(), chain)] {
        assert!(path.join("rocksdb/CURRENT").is_file());
        run(reth_ok, "db", path, spec, &["drop", "--force"]);
    }
}

#[cfg(unix)]
#[test]
fn requires_readable_rocksdb_directory() {
    let node = tempfile::tempdir().unwrap();
    let dir = node.path();
    run(reth_ok, "init", dir, "dev", &[]);
    let rocks = dir.join("rocksdb");
    let original = rocks.metadata().unwrap().permissions();
    fs::set_permissions(&rocks, fs::Permissions::from_mode(0o300)).unwrap();
    // Privileged test runners may still enumerate mode 0300.
    if let Err(err) = fs::read_dir(&rocks) {
        assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
        assert_eq!(
            run(reth_error, "db", dir, "dev", &["drop", "--force"]),
            format!("Error: failed to read dir {rocks:?}: {err}")
        );
        assert!(dir.join("db/mdbx.dat").is_file());
        assert!(rocks.join("CURRENT").is_file());
    }
    fs::set_permissions(&rocks, original).unwrap();
    run(reth_ok, "db", dir, "dev", &["drop", "--force"]);
}
