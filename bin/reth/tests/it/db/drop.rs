use super::*;
use clap::Parser;
use reth_chainspec::{DEV, SEPOLIA};
use reth_cli_runner::CliContext;
use reth_ethereum_cli::{Cli, Commands};
use reth_node_ethereum::EthereumNode;
use reth_tasks::Runtime;
use std::error::Error;

const DROP: &[&str] = &["drop", "--force"];

#[test]
fn cleans_custom_rocksdb_history() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("node");
    let rocks = temp.path().join("external");
    let old = Address::ZERO.to_string();
    let old_chain = json!({
        "config": { "chainId": DEV.chain.id() },
        "alloc": { (&old): { "balance": "0x1" } }
    })
    .to_string();
    let invoke = |cmd, chain: &str, args: &[&str]| {
        let options = [
            cmd,
            "--datadir",
            dir.to_str().unwrap(),
            "--datadir.rocksdb",
            rocks.to_str().unwrap(),
            "--chain",
            chain,
        ];
        reth_ok(&[&options[..], args].concat())
    };
    let history = |chain| invoke("db", chain, &["get", "rocksdb", "accounts-history", &old]);
    invoke("init", &old_chain, &[]);
    assert_eq!(serde_json::from_str::<Vec<u64>>(&history(&old_chain)).unwrap(), [0]);
    invoke("db", &old_chain, DROP);
    invoke("init", "dev", &[]);
    assert_eq!(history("dev"), "");
}

#[tokio::test]
async fn verifies_both_stores_before_dropping_a_legacy_database() {
    let temp = tempfile::tempdir().unwrap();
    let dev = temp.path().join("dev");
    let other = temp.path().join("other");
    execute("init", &dev, "dev", &[]).await.unwrap();
    execute("init", &other, "sepolia", &["--storage.v2=false"]).await.unwrap();
    let static_files = dev.join("static_files");
    let args = ["--datadir.static-files", static_files.to_str().unwrap()];

    // Only the static files match dev; only MDBX matches sepolia.
    assert_eq!(
        drop_error(&other, "dev", &args).await,
        format!("Database does not contain the configured genesis {}", DEV.genesis_hash())
    );
    assert_eq!(
        drop_error(&other, "sepolia", &args).await,
        format!(
            "Static files at {} do not contain the configured genesis {}",
            static_files.display(),
            SEPOLIA.genesis_hash()
        )
    );
    assert!(other.join("db/mdbx.dat").is_file());
    assert!(other.join("rocksdb/CURRENT").is_file());

    // Legacy datadirs can predate RocksDB.
    fs::remove_dir_all(other.join("rocksdb")).unwrap();
    execute("db", &other, "sepolia", DROP).await.unwrap();
    assert!(!other.join("db").exists());
    assert!(!other.join("rocksdb").exists());
}

#[tokio::test]
async fn refuses_unavailable_static_files_override() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("node");
    let volume = temp.path().join("volume");
    let static_files = volume.join("static");
    let args = ["--datadir.static-files", static_files.to_str().unwrap()];
    execute("init", &dir, "dev", &args).await.unwrap();
    fs::rename(&volume, temp.path().join("offline")).unwrap();

    let expected = format!("No genesis in configured static files at {}", static_files.display());
    assert_eq!(drop_error(&dir, "dev", &args).await, expected);
    assert!(!volume.exists());
    fs::create_dir_all(&static_files).unwrap();
    assert_eq!(drop_error(&dir, "dev", &args).await, expected);
    assert!(dir.join("db/mdbx.dat").is_file());
    assert!(dir.join("rocksdb/CURRENT").is_file());

    fs::remove_dir_all(&volume).unwrap();
    fs::rename(temp.path().join("offline"), &volume).unwrap();
    execute("db", &dir, "dev", &[&args[..], DROP].concat()).await.unwrap();
    assert!(!dir.join("db").exists());
    assert!(!dir.join("rocksdb/CURRENT").exists());
    assert_eq!(fs::read_dir(&static_files).unwrap().count(), 0);
}

async fn drop_error(dir: &Path, chain: &str, args: &[&str]) -> String {
    execute("db", dir, chain, &[args, DROP].concat()).await.unwrap_err().to_string()
}

async fn execute(
    cmd: &str,
    dir: &Path,
    chain: &str,
    args: &[&str],
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let options = ["reth", cmd, "--datadir", dir.to_str().unwrap(), "--chain", chain];
    let cli: Cli = Parser::try_parse_from(options.into_iter().chain(args.iter().copied()))?;
    let runtime = Runtime::test();
    match cli.command {
        Commands::Init(init) => init.execute::<EthereumNode>(runtime).await?,
        Commands::Db(db) => {
            db.execute::<EthereumNode>(CliContext { task_executor: runtime }).await?
        }
        _ => unreachable!(),
    }
    Ok(())
}
