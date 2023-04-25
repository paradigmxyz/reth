//! Command for clearing redundant changesets.
use crate::dirs::{DbPath, MaybePlatformPath};
use clap::Parser;
use reth_db::{
    cursor::{DbCursorRO, DbDupCursorRO},
    models::{storage_sharded_key::StorageShardedKey, BlockNumberAddress},
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_primitives::{ChainSpec, H256};
use reth_provider::Transaction;
use reth_staged_sync::utils::{chainspec::genesis_value_parser, init::init_db};
use std::{collections::HashSet, sync::Arc, time::Instant};

/// `reth prune-changesets` command
/// Ref <https://github.com/paradigmxyz/reth/pull/2355>
#[derive(Debug, Parser)]
pub struct Command {
    /// The path to the database folder.
    ///
    /// Defaults to the OS-specific data directory:
    ///
    /// - Linux: `$XDG_DATA_HOME/reth/db` or `$HOME/.local/share/reth/db`
    /// - Windows: `{FOLDERID_RoamingAppData}/reth/db`
    /// - macOS: `$HOME/Library/Application Support/reth/db`
    #[arg(global = true, long, value_name = "PATH", verbatim_doc_comment, default_value_t)]
    db: MaybePlatformPath<DbPath>,

    /// The chain this node is running.
    ///
    /// Possible values are either a built-in chain or the path to a chain specification file.
    ///
    /// Built-in chains:
    /// - mainnet
    /// - goerli
    /// - sepolia
    #[arg(
        long,
        value_name = "CHAIN_OR_PATH",
        verbatim_doc_comment,
        default_value = "mainnet",
        value_parser = genesis_value_parser
    )]
    chain: Arc<ChainSpec>,

    /// Flag wether to commit changes or not
    commit: bool,
}

impl Command {
    /// Execute `prune-changesets` command
    pub async fn execute(self) -> eyre::Result<()> {
        // add network name to db directory
        let db_path = self.db.unwrap_or_chain_default(self.chain.chain);

        std::fs::create_dir_all(&db_path)?;

        let db = Arc::new(init_db(db_path)?);
        let tx = Transaction::new(db.as_ref())?;

        let mut storage_changesets_cursor = tx.cursor_dup_write::<tables::StorageChangeSet>()?;

        // This might OOM
        let mut all_keys_to_delete =
            HashSet::<(BlockNumberAddress, H256)>::with_capacity(1_000_000);

        let mut current_entry = storage_changesets_cursor.first()?;
        let instant = Instant::now();
        while let Some((key, entry)) = current_entry {
            if all_keys_to_delete.contains(&(key, entry.key)) {
                current_entry = storage_changesets_cursor.next()?;
                continue
            }

            // Seek all next changesets with the same value
            let entry_instant = Instant::now();
            let mut keys_to_delete = Vec::new();

            // history key to search IntegerList of block changesets.
            let mut next_block = Some(key.0 .0 + 1);

            while let Some(next_block_number) = next_block {
                let history_key = StorageShardedKey::new(key.0 .1, entry.key, next_block_number);
                let changeset_block_number = tx
                    .cursor_read::<tables::StorageHistory>()?
                    .seek(history_key)?
                    .filter(|(k, _)| k.address == key.0 .1 && k.sharded_key.key == entry.key)
                    .map(|(_, list)| {
                        list.0.enable_rank().successor(next_block_number as usize).map(|i| i as u64)
                    });

                if let Some(Some(changeset_block_number)) = changeset_block_number {
                    let storage_changset = tx
                        .cursor_dup_read::<tables::StorageChangeSet>()?
                        .seek_by_key_subkey((changeset_block_number, key.0 .1).into(), entry.key)?
                        .filter(|e| e.key == entry.key)
                        .ok_or(eyre::eyre!("expected changset entry"))?;

                    if storage_changset.value == entry.value {
                        // if value is the same we can delete changeset
                        keys_to_delete.push(key);
                        next_block = Some(changeset_block_number + 1);
                    } else {
                        next_block = None;
                    }
                } else {
                    // if changeset is not present that means that there was history shard but we
                    // need to use newest value from plain state
                    let plain_state_entry = tx
                        .cursor_dup_read::<tables::PlainStorageState>()?
                        .seek_by_key_subkey(key.0 .1, entry.key)?
                        .filter(|e| e.key == entry.key && e.value == entry.value);
                    if plain_state_entry.is_some() {
                        // if value is the same we can delete changeset
                        keys_to_delete.push(key);
                    }
                    next_block = None;
                }
            }

            if !keys_to_delete.is_empty() {
                println!(
                    "Block #{}: Found {} changesets to delete for {:?} at slot {:?} in {} seconds",
                    key.0 .0,
                    keys_to_delete.len(),
                    key.0 .1,
                    entry.key,
                    entry_instant.elapsed().as_secs()
                );
                all_keys_to_delete.extend(keys_to_delete.into_iter().map(|key| (key, entry.key)));
                println!("Total Len: {}", all_keys_to_delete.len());
            }
            current_entry = storage_changesets_cursor.next()?;
        }

        println!(
            "Found {} changesets to delete in {} seconds",
            all_keys_to_delete.len(),
            instant.elapsed().as_secs()
        );

        // TODO: recalculate state root to ensure changes are correct

        if self.commit {
            // commit changes
        }

        Ok(())
    }
}
