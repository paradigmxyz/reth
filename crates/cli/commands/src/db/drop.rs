//! Destructive cleanup of the configured database stores.

use crate::common::EnvironmentArgs;
use alloy_primitives::{BlockHash, BlockNumber};
use eyre::{Result, WrapErr};
use reth_chainspec::EthChainSpec;
use reth_cli::chainspec::ChainSpecParser;
use reth_db::{lockfile::StorageLock, open_db_read_only};
use reth_db_api::{database::Database, tables::HeaderNumbers, transaction::DbTx};
use reth_fs_util as fs;
use reth_node_builder::NodePrimitives;
use reth_node_core::dirs::{ChainPath, DataDirPath};
use reth_provider::{
    providers::{RocksDBProvider, StaticFileProviderBuilder},
    BlockHashReader,
};
use std::{
    io::{self, Write},
    path::{self, Path, PathBuf},
};
use tracing::info;

pub(super) fn execute<P: NodePrimitives, C: ChainSpecParser<ChainSpec: EthChainSpec>>(
    env: &EnvironmentArgs<C>,
    data_dir: &ChainPath<DataDirPath>,
    force: bool,
) -> Result<()> {
    let db = path::absolute(data_dir.db())?;
    let rocks = path::absolute(data_dir.rocksdb())?;
    let static_files = path::absolute(data_dir.static_files())?;
    let wal = path::absolute(data_dir.exex_wal())?;

    if !force {
        print!(
            "Permanently delete data at these paths?\n  MDBX: {}\n  RocksDB (dedicated directory): {}\n  Static files: {}\n  ExEx WAL: {}\nThis cannot be undone. (y/N): ",
            db.display(), rocks.display(), static_files.display(), wal.display()
        );
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Database drop aborted!");
            return Ok(())
        }
    }

    check_store_paths([&db, &static_files, &rocks, &wal])?;

    // A read-write environment can initialize missing stores and write genesis. Drop must verify
    // the stores as found on disk before deleting any of them.
    let _db_lock = StorageLock::try_acquire(&db.canonicalize()?)
        .wrap_err_with(|| format!("Could not lock {}", db.display()))?;
    let hash = env.chain.genesis_hash();
    let number = env.chain.genesis().number.unwrap_or_default();
    let static_lock = lock_static_files::<P>(
        &static_files,
        env.datadir.static_files_path.is_some(),
        number,
        hash,
    )?;
    if db.join("mdbx.dat").try_exists()? {
        let store = open_db_read_only(&db, env.db.database_args())?;
        let tx = store.tx()?;
        eyre::ensure!(
            tx.get::<HeaderNumbers>(hash)? == Some(number) ||
                (static_lock.is_some() && tx.entries::<HeaderNumbers>()? == 0),
            "Database does not contain the configured genesis {hash}"
        );
    }

    info!(target: "reth::cli", ?db, ?rocks, ?static_files, ?wal, "Dropping database stores");
    if rocks.try_exists()? {
        RocksDBProvider::destroy(&rocks)?;
    }
    if wal.try_exists()? {
        fs::remove_dir_all(&wal)?;
    }
    if static_lock.is_some() {
        fs::remove_dir_all(&static_files)?;
    }
    fs::create_dir_all(&static_files)?;
    fs::remove_dir_all(&db)?;
    Ok(())
}

/// Returns the lock for a verified static store, or `None` for an empty or absent default store.
fn lock_static_files<P: NodePrimitives>(
    path: &Path,
    require_genesis: bool,
    number: BlockNumber,
    hash: BlockHash,
) -> Result<Option<StorageLock>> {
    if path.try_exists()? {
        let lock = StorageLock::try_acquire(&path.canonicalize()?)
            .wrap_err_with(|| format!("Could not lock {}", path.display()))?;
        let stored = StaticFileProviderBuilder::read_only(path)
            .with_genesis_block_number(number)
            .build::<P>()?
            .block_hash(number)?;
        if let Some(stored) = stored {
            eyre::ensure!(
                stored == hash,
                "Static files at {} do not contain the configured genesis {hash}",
                path.display()
            );
            return Ok(Some(lock))
        }
        // Release the writer lock so an otherwise empty directory is visible as empty.
        drop(lock);
        eyre::ensure!(
            fs::read_dir(path)?.next().transpose()?.is_none(),
            "Missing genesis in nonempty static files at {}",
            path.display()
        );
    }

    eyre::ensure!(!require_genesis, "No genesis in configured static files at {}", path.display());
    Ok(None)
}

/// Reject overlapping stores and paths hidden by a dangling symlink.
fn check_store_paths(paths: [&Path; 4]) -> Result<()> {
    let mut roots = Vec::<PathBuf>::new();
    for path in paths {
        if path.try_exists()? {
            let root = path
                .canonicalize()
                .wrap_err_with(|| format!("Could not resolve {}", path.display()))?;
            for other in &roots {
                eyre::ensure!(
                    !root.starts_with(other) && !other.starts_with(&root),
                    "Database directories overlap: {root:?} and {other:?}"
                );
            }
            roots.push(root);
        } else {
            // A missing store is valid, but a dangling symlink can hide an existing one.
            for prefix in path.ancestors() {
                if prefix.try_exists()? {
                    break
                }
                eyre::ensure!(!prefix.is_symlink(), "Could not resolve {}", prefix.display());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    use std::os::unix::fs::symlink;

    #[test]
    fn rejects_overlapping_store_paths() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().canonicalize().unwrap();
        let db = dir.join("db");
        fs::create_dir_all(&db).unwrap();
        let err = check_store_paths([&db, &dir.join("static_files"), &dir, &dir.join("exex/wal")])
            .unwrap_err();
        assert_eq!(err.to_string(), format!("Database directories overlap: {dir:?} and {db:?}"));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_dangling_store_paths() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path();
        let link = dir.join("volume");
        symlink(dir.join("offline"), &link).unwrap();
        for rocks in [&link, &link.join("rocksdb")] {
            let err = check_store_paths([
                &dir.join("db"),
                &dir.join("static_files"),
                rocks,
                &dir.join("exex/wal"),
            ])
            .unwrap_err();
            assert_eq!(err.to_string(), format!("Could not resolve {}", link.display()));
        }
    }
}
