use alloy_primitives::{hex, Address, BlockHash, B256};
use clap::Parser;
use reth_db::{
    static_file::{
        AccountChangesetMask, ColumnSelectorOne, ColumnSelectorTwo, HeaderWithHashMask,
        ReceiptMask, TransactionMask, TransactionSenderMask,
    },
    RawDupSort,
};
use reth_db_api::{
    cursor::{DbCursorRO, DbDupCursorRO},
    database::Database,
    models::{storage_sharded_key::StorageShardedKey, ShardedKey},
    table::{Compress, Decompress, DupSort, Table},
    tables,
    transaction::DbTx,
    RawKey, RawTable, Receipts, TableViewer, Transactions,
};
use reth_db_common::DbTool;
use reth_node_api::{HeaderTy, ReceiptTy, TxTy};
use reth_node_builder::NodeTypesWithDB;
use reth_primitives_traits::ValueWithSubKey;
use reth_provider::{
    providers::ProviderNodeTypes, ChangeSetReader, RocksDBProviderFactory,
    StaticFileProviderFactory,
};
use reth_static_file_types::StaticFileSegment;
use reth_storage_api::StorageChangeSetReader;
use tracing::error;

/// The arguments for the `reth db get` command
#[derive(Parser, Debug)]
pub struct Command {
    #[command(subcommand)]
    subcommand: Subcommand,
}

#[derive(clap::Subcommand, Debug)]
enum Subcommand {
    /// Gets the content of a database table for the given key
    Mdbx {
        table: tables::Tables,

        /// The key to get content for
        #[arg(value_parser = maybe_json_value_parser)]
        key: String,

        /// The subkey to get content for
        #[arg(value_parser = maybe_json_value_parser)]
        subkey: Option<String>,

        /// Optional end key for range query (exclusive upper bound)
        #[arg(value_parser = maybe_json_value_parser)]
        end_key: Option<String>,

        /// Optional end subkey for range query (exclusive upper bound)
        #[arg(value_parser = maybe_json_value_parser)]
        end_subkey: Option<String>,

        /// Output bytes instead of human-readable decoded value
        #[arg(long)]
        raw: bool,
    },
    /// Gets the content of a static file segment for the given key
    StaticFile {
        segment: StaticFileSegment,

        /// The key to get content for
        #[arg(value_parser = maybe_json_value_parser)]
        key: String,

        /// The subkey to get content for, for example address in changeset
        #[arg(value_parser = maybe_json_value_parser)]
        subkey: Option<String>,

        /// Output bytes instead of human-readable decoded value
        #[arg(long)]
        raw: bool,
    },
    /// Gets the content of a RocksDB table for the given key
    ///
    /// For history tables (accounts-history, storages-history), you can pass a plain address
    /// instead of a full JSON ShardedKey. Use --block to query a specific block number
    /// (seeks to the shard containing that block), or --all-shards to list all shards for
    /// the address.
    ///
    /// Examples:
    ///   reth db get rocksdb accounts-history 0xdBBE3D8c2d2b22A2611c5A94A9a12C2fCD49Eb29
    ///   reth db get rocksdb accounts-history 0xdBBE...Eb29 --block 1000000
    ///   reth db get rocksdb accounts-history 0xdBBE...Eb29 --all-shards
    ///   reth db get rocksdb storages-history 0xdBBE...Eb29 --storage-key 0x0000...0003
    Rocksdb {
        /// The RocksDB table
        #[arg(value_enum)]
        table: RocksDbTable,

        /// The key to get content for. For history tables, this can be a plain address.
        #[arg(value_parser = maybe_json_value_parser)]
        key: String,

        /// Target block number for history tables. Seeks to the shard containing this block.
        /// Defaults to the latest shard if not specified.
        #[arg(long)]
        block: Option<u64>,

        /// Storage key for storages-history table lookups.
        #[arg(long)]
        storage_key: Option<String>,

        /// List all shards for the given key (history tables only).
        #[arg(long)]
        all_shards: bool,

        /// Output bytes instead of human-readable decoded value
        #[arg(long)]
        raw: bool,
    },
}

/// RocksDB tables that can be queried.
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum RocksDbTable {
    /// Transaction hash to transaction number mapping
    TransactionHashNumbers,
    /// Account history indices
    AccountsHistory,
    /// Storage history indices
    StoragesHistory,
}

impl Command {
    /// Execute `db get` command
    pub fn execute<N: ProviderNodeTypes>(self, tool: &DbTool<N>) -> eyre::Result<()> {
        match self.subcommand {
            Subcommand::Mdbx { table, key, subkey, end_key, end_subkey, raw } => {
                table.view(&GetValueViewer { tool, key, subkey, end_key, end_subkey, raw })?
            }
            Subcommand::Rocksdb { table, key, block, storage_key, all_shards, raw } => {
                get_rocksdb(tool, table, &key, block, storage_key.as_deref(), all_shards, raw)?;
            }
            Subcommand::StaticFile { segment, key, subkey, raw } => {
                if let StaticFileSegment::StorageChangeSets = segment {
                    let storage_key =
                        table_subkey::<tables::StorageChangeSets>(subkey.as_deref()).ok();
                    let key = table_key::<tables::StorageChangeSets>(&key)?;

                    let provider = tool.provider_factory.static_file_provider();

                    if let Some(storage_key) = storage_key {
                        let entry = provider.get_storage_before_block(
                            key.block_number(),
                            key.address(),
                            storage_key,
                        )?;

                        if let Some(entry) = entry {
                            let se: reth_primitives_traits::StorageEntry = entry;
                            println!("{}", serde_json::to_string_pretty(&se)?);
                        } else {
                            error!(target: "reth::cli", "No content for the given table key.");
                        }
                        return Ok(());
                    }

                    let changesets = provider.storage_changeset(key.block_number())?;
                    let serializable: Vec<_> = changesets
                        .into_iter()
                        .map(|(addr, entry)| {
                            let se: reth_primitives_traits::StorageEntry = entry;
                            (addr, se)
                        })
                        .collect();
                    println!("{}", serde_json::to_string_pretty(&serializable)?);
                    return Ok(());
                }

                let (key, subkey, mask): (u64, _, _) = match segment {
                    StaticFileSegment::Headers => (
                        table_key::<tables::Headers>(&key)?,
                        None,
                        <HeaderWithHashMask<HeaderTy<N>>>::MASK,
                    ),
                    StaticFileSegment::Transactions => (
                        table_key::<tables::Transactions>(&key)?,
                        None,
                        <TransactionMask<TxTy<N>>>::MASK,
                    ),
                    StaticFileSegment::Receipts => (
                        table_key::<tables::Receipts>(&key)?,
                        None,
                        <ReceiptMask<ReceiptTy<N>>>::MASK,
                    ),
                    StaticFileSegment::TransactionSenders => (
                        table_key::<tables::TransactionSenders>(&key)?,
                        None,
                        TransactionSenderMask::MASK,
                    ),
                    StaticFileSegment::AccountChangeSets => {
                        let subkey =
                            table_subkey::<tables::AccountChangeSets>(subkey.as_deref()).ok();
                        (
                            table_key::<tables::AccountChangeSets>(&key)?,
                            subkey,
                            AccountChangesetMask::MASK,
                        )
                    }
                    StaticFileSegment::StorageChangeSets => {
                        unreachable!("storage changesets handled above");
                    }
                };

                // handle account changesets differently if a subkey is provided.
                if let StaticFileSegment::AccountChangeSets = segment {
                    let Some(subkey) = subkey else {
                        // get all changesets for the block
                        let changesets = tool
                            .provider_factory
                            .static_file_provider()
                            .account_block_changeset(key)?;

                        println!("{}", serde_json::to_string_pretty(&changesets)?);
                        return Ok(())
                    };

                    let account = tool
                        .provider_factory
                        .static_file_provider()
                        .get_account_before_block(key, subkey)?;

                    if let Some(account) = account {
                        println!("{}", serde_json::to_string_pretty(&account)?);
                    } else {
                        error!(target: "reth::cli", "No content for the given table key.");
                    }

                    return Ok(())
                }

                let content = tool.provider_factory.static_file_provider().find_static_file(
                    segment,
                    |provider| {
                        let mut cursor = provider.cursor()?;
                        cursor.get(key.into(), mask).map(|result| {
                            result.map(|vec| {
                                vec.iter().map(|slice| slice.to_vec()).collect::<Vec<_>>()
                            })
                        })
                    },
                )?;

                match content {
                    Some(content) => {
                        if raw {
                            println!("{}", hex::encode_prefixed(&content[0]));
                        } else {
                            match segment {
                                StaticFileSegment::Headers => {
                                    let header = HeaderTy::<N>::decompress(content[0].as_slice())?;
                                    let block_hash = BlockHash::decompress(content[1].as_slice())?;
                                    println!(
                                        "Header\n{}\n\nBlockHash\n{}",
                                        serde_json::to_string_pretty(&header)?,
                                        serde_json::to_string_pretty(&block_hash)?
                                    );
                                }
                                StaticFileSegment::Transactions => {
                                    let transaction = <<Transactions as Table>::Value>::decompress(
                                        content[0].as_slice(),
                                    )?;
                                    println!("{}", serde_json::to_string_pretty(&transaction)?);
                                }
                                StaticFileSegment::Receipts => {
                                    let receipt = <<Receipts as Table>::Value>::decompress(
                                        content[0].as_slice(),
                                    )?;
                                    println!("{}", serde_json::to_string_pretty(&receipt)?);
                                }
                                StaticFileSegment::TransactionSenders => {
                                    let sender =
                                        <<tables::TransactionSenders as Table>::Value>::decompress(
                                            content[0].as_slice(),
                                        )?;
                                    println!("{}", serde_json::to_string_pretty(&sender)?);
                                }
                                StaticFileSegment::AccountChangeSets => {
                                    unreachable!("account changeset static files are special cased before this match")
                                }
                                StaticFileSegment::StorageChangeSets => {
                                    unreachable!("storage changeset static files are special cased before this match")
                                }
                            }
                        }
                    }
                    None => {
                        error!(target: "reth::cli", "No content for the given table key.");
                    }
                };
            }
        }

        Ok(())
    }
}

/// Gets a value from a RocksDB table by key.
fn get_rocksdb<N: ProviderNodeTypes>(
    tool: &DbTool<N>,
    table: RocksDbTable,
    key: &str,
    block: Option<u64>,
    storage_key: Option<&str>,
    all_shards: bool,
    raw: bool,
) -> eyre::Result<()> {
    let rocksdb = tool.provider_factory.rocksdb_provider();

    match table {
        RocksDbTable::TransactionHashNumbers => {
            if block.is_some() || all_shards || storage_key.is_some() {
                return Err(eyre::eyre!(
                    "--block, --all-shards, and --storage-key are only supported for history tables"
                ));
            }
            get_rocksdb_table::<tables::TransactionHashNumbers>(&rocksdb, key, raw)
        }
        RocksDbTable::AccountsHistory => {
            if storage_key.is_some() {
                return Err(eyre::eyre!("--storage-key is only supported for storages-history"));
            }
            get_rocksdb_account_history(&rocksdb, key, block, all_shards, raw)
        }
        RocksDbTable::StoragesHistory => {
            get_rocksdb_storage_history(&rocksdb, key, storage_key, block, all_shards, raw)
        }
    }
}

/// Try to parse a key string as a plain address, falling back to JSON `ShardedKey` parsing.
fn parse_address(key: &str) -> eyre::Result<Address> {
    // Strip surrounding quotes that `maybe_json_value_parser` may have added
    let stripped = key.trim_matches('"');
    stripped.parse::<Address>().map_err(|e| eyre::eyre!("failed to parse address: {e}"))
}

/// Gets account history from RocksDB with ergonomic key parsing.
///
/// Accepts a plain address and uses seek to find the relevant shard.
fn get_rocksdb_account_history(
    rocksdb: &reth_provider::providers::RocksDBProvider,
    key: &str,
    block: Option<u64>,
    all_shards: bool,
    raw: bool,
) -> eyre::Result<()> {
    // Try parsing as a plain address first, fall back to full JSON ShardedKey
    match parse_address(key) {
        Ok(address) => {
            let block_number = block.unwrap_or(u64::MAX);
            let seek_key = ShardedKey::new(address, block_number);

            if all_shards {
                // Iterate all shards: seek from (address, 0) until address changes
                let start = ShardedKey::new(address, 0);
                let iter = rocksdb.iter_from::<tables::AccountsHistory>(start)?;
                for result in iter {
                    let (k, v) = result?;
                    if k.key != address {
                        break;
                    }
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "highest_block_number": k.highest_block_number,
                            "value": v,
                        }))?
                    );
                }
            } else {
                // Seek to the first shard with highest_block_number >= target
                let mut iter = rocksdb.iter_from::<tables::AccountsHistory>(seek_key)?;
                match iter.next() {
                    Some(Ok((k, v))) if k.key == address => {
                        if raw {
                            let raw_val = rocksdb.get_raw::<tables::AccountsHistory>(k)?;
                            if let Some(bytes) = raw_val {
                                println!("{}", hex::encode_prefixed(&bytes));
                            }
                        } else {
                            println!("{}", serde_json::to_string_pretty(&v)?);
                        }
                    }
                    _ => {
                        error!(target: "reth::cli", "No content for the given table key.");
                    }
                }
            }
            Ok(())
        }
        Err(_) => {
            // Fall back to full JSON key parsing (e.g.
            // `{"key":"0x...","highest_block_number":...}`)
            if all_shards || block.is_some() {
                return Err(eyre::eyre!(
                    "--block and --all-shards require a plain address, not a JSON key"
                ));
            }
            get_rocksdb_table::<tables::AccountsHistory>(rocksdb, key, raw)
        }
    }
}

/// Gets storage history from RocksDB with ergonomic key parsing.
///
/// Accepts a plain address + optional `--storage-key` and uses seek.
fn get_rocksdb_storage_history(
    rocksdb: &reth_provider::providers::RocksDBProvider,
    key: &str,
    storage_key: Option<&str>,
    block: Option<u64>,
    all_shards: bool,
    raw: bool,
) -> eyre::Result<()> {
    match parse_address(key) {
        Ok(address) => {
            let storage_key = storage_key
                .map(|s| s.trim_matches('"').parse::<B256>())
                .transpose()
                .map_err(|e| eyre::eyre!("failed to parse storage key: {e}"))?
                .unwrap_or_default();
            let block_number = block.unwrap_or(u64::MAX);
            let seek_key = StorageShardedKey::new(address, storage_key, block_number);

            if all_shards {
                let start = StorageShardedKey::new(address, storage_key, 0);
                let iter = rocksdb.iter_from::<tables::StoragesHistory>(start)?;
                for result in iter {
                    let (k, v) = result?;
                    if k.address != address || k.sharded_key.key != storage_key {
                        break;
                    }
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "highest_block_number": k.sharded_key.highest_block_number,
                            "value": v,
                        }))?
                    );
                }
            } else {
                let mut iter = rocksdb.iter_from::<tables::StoragesHistory>(seek_key)?;
                match iter.next() {
                    Some(Ok((k, v)))
                        if k.address == address && k.sharded_key.key == storage_key =>
                    {
                        if raw {
                            let raw_val = rocksdb.get_raw::<tables::StoragesHistory>(k)?;
                            if let Some(bytes) = raw_val {
                                println!("{}", hex::encode_prefixed(&bytes));
                            }
                        } else {
                            println!("{}", serde_json::to_string_pretty(&v)?);
                        }
                    }
                    _ => {
                        error!(target: "reth::cli", "No content for the given table key.");
                    }
                }
            }
            Ok(())
        }
        Err(_) => {
            if all_shards || block.is_some() || storage_key.is_some() {
                return Err(eyre::eyre!(
                    "--block, --all-shards, and --storage-key require a plain address, not a JSON key"
                ));
            }
            get_rocksdb_table::<tables::StoragesHistory>(rocksdb, key, raw)
        }
    }
}

/// Gets a value from a specific RocksDB table by exact key and prints it.
fn get_rocksdb_table<T: Table>(
    rocksdb: &reth_provider::providers::RocksDBProvider,
    key_str: &str,
    raw: bool,
) -> eyre::Result<()> {
    let key = table_key::<T>(key_str)?;

    if raw {
        let content = rocksdb.get_raw::<T>(key)?;
        match content {
            Some(bytes) => println!("{}", hex::encode_prefixed(&bytes)),
            None => error!(target: "reth::cli", "No content for the given table key."),
        }
    } else {
        let content = rocksdb.get::<T>(key)?;
        match content {
            Some(value) => println!("{}", serde_json::to_string_pretty(&value)?),
            None => error!(target: "reth::cli", "No content for the given table key."),
        }
    }

    Ok(())
}

/// Get an instance of key for given table
pub(crate) fn table_key<T: Table>(key: &str) -> Result<T::Key, eyre::Error> {
    serde_json::from_str(key).map_err(|e| eyre::eyre!(e))
}

/// Get an instance of subkey for given dupsort table
fn table_subkey<T: DupSort>(subkey: Option<&str>) -> Result<T::SubKey, eyre::Error> {
    serde_json::from_str(subkey.unwrap_or_default()).map_err(|e| eyre::eyre!(e))
}

struct GetValueViewer<'a, N: NodeTypesWithDB> {
    tool: &'a DbTool<N>,
    key: String,
    subkey: Option<String>,
    end_key: Option<String>,
    end_subkey: Option<String>,
    raw: bool,
}

impl<N: ProviderNodeTypes> TableViewer<()> for GetValueViewer<'_, N> {
    type Error = eyre::Report;

    fn view<T: Table>(&self) -> Result<(), Self::Error> {
        let key = table_key::<T>(&self.key)?;

        // A non-dupsort table cannot have subkeys. The `subkey` arg becomes the `end_key`. First we
        // check that `end_key` and `end_subkey` weren't previously given, as that wouldn't be
        // valid.
        if self.end_key.is_some() || self.end_subkey.is_some() {
            return Err(eyre::eyre!("Only END_KEY can be given for non-DUPSORT tables"));
        }

        let end_key = self.subkey.clone();

        // Check if we're doing a range query
        if let Some(ref end_key_str) = end_key {
            let end_key = table_key::<T>(end_key_str)?;

            // Use walk_range to iterate over the range
            self.tool.provider_factory.db_ref().view(|tx| {
                let mut cursor = tx.cursor_read::<T>()?;
                let walker = cursor.walk_range(key..end_key)?;

                for result in walker {
                    let (k, v) = result?;
                    let json_val = if self.raw {
                        let raw_key = RawKey::from(k);
                        serde_json::json!({
                            "key": hex::encode_prefixed(raw_key.raw_key()),
                            "val": hex::encode_prefixed(v.compress().as_ref()),
                        })
                    } else {
                        serde_json::json!({
                            "key": &k,
                            "val": &v,
                        })
                    };

                    println!("{}", serde_json::to_string_pretty(&json_val)?);
                }

                Ok::<_, eyre::Report>(())
            })??;
        } else {
            // Single key lookup
            let content = if self.raw {
                self.tool
                    .get::<RawTable<T>>(RawKey::from(key))?
                    .map(|content| hex::encode_prefixed(content.raw_value()))
            } else {
                self.tool.get::<T>(key)?.as_ref().map(serde_json::to_string_pretty).transpose()?
            };

            match content {
                Some(content) => {
                    println!("{content}");
                }
                None => {
                    error!(target: "reth::cli", "No content for the given table key.");
                }
            };
        }

        Ok(())
    }

    fn view_dupsort<T: DupSort>(&self) -> Result<(), Self::Error>
    where
        T::Value: reth_primitives_traits::ValueWithSubKey<SubKey = T::SubKey>,
    {
        // get a key for given table
        let key = table_key::<T>(&self.key)?;

        // Check if we're doing a range query
        if let Some(ref end_key_str) = self.end_key {
            let end_key = table_key::<T>(end_key_str)?;
            let start_subkey = table_subkey::<T>(Some(
                self.subkey.as_ref().expect("must have been given if end_key is given").as_str(),
            ))?;
            let end_subkey_parsed = self
                .end_subkey
                .as_ref()
                .map(|s| table_subkey::<T>(Some(s.as_str())))
                .transpose()?;

            self.tool.provider_factory.db_ref().view(|tx| {
                let mut cursor = tx.cursor_dup_read::<T>()?;

                // Seek to the starting key. If there is actually a key at the starting key then
                // seek to the subkey within it.
                if let Some((decoded_key, _)) = cursor.seek(key.clone())? &&
                    decoded_key == key
                {
                    cursor.seek_by_key_subkey(key.clone(), start_subkey.clone())?;
                }

                // Get the current position to start iteration
                let mut current = cursor.current()?;

                while let Some((decoded_key, decoded_value)) = current {
                    // Extract the subkey using the ValueWithSubKey trait
                    let decoded_subkey = decoded_value.get_subkey();

                    // Check if we've reached the end (exclusive)
                    if (&decoded_key, Some(&decoded_subkey)) >=
                        (&end_key, end_subkey_parsed.as_ref())
                    {
                        break;
                    }

                    // Output the entry with both key and subkey
                    let json_val = if self.raw {
                        let raw_key = RawKey::from(decoded_key.clone());
                        serde_json::json!({
                            "key": hex::encode_prefixed(raw_key.raw_key()),
                            "val": hex::encode_prefixed(decoded_value.compress().as_ref()),
                        })
                    } else {
                        serde_json::json!({
                            "key": &decoded_key,
                            "val": &decoded_value,
                        })
                    };

                    println!("{}", serde_json::to_string_pretty(&json_val)?);

                    // Move to next entry
                    current = cursor.next()?;
                }

                Ok::<_, eyre::Report>(())
            })??;
        } else {
            // Single key/subkey lookup
            let subkey = table_subkey::<T>(self.subkey.as_deref())?;

            let content = if self.raw {
                self.tool
                    .get_dup::<RawDupSort<T>>(RawKey::from(key), RawKey::from(subkey))?
                    .map(|content| hex::encode_prefixed(content.raw_value()))
            } else {
                self.tool
                    .get_dup::<T>(key, subkey)?
                    .as_ref()
                    .map(serde_json::to_string_pretty)
                    .transpose()?
            };

            match content {
                Some(content) => {
                    println!("{content}");
                }
                None => {
                    error!(target: "reth::cli", "No content for the given table subkey.");
                }
            };
        }
        Ok(())
    }
}

/// Map the user input value to json
pub(crate) fn maybe_json_value_parser(value: &str) -> Result<String, eyre::Error> {
    if serde_json::from_str::<serde::de::IgnoredAny>(value).is_ok() {
        Ok(value.to_string())
    } else {
        serde_json::to_string(&value).map_err(|e| eyre::eyre!(e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{address, B256};
    use clap::{Args, Parser};
    use reth_db_api::{
        models::{storage_sharded_key::StorageShardedKey, ShardedKey},
        AccountsHistory, HashedAccounts, Headers, StageCheckpoints, StoragesHistory,
    };
    use std::str::FromStr;

    /// A helper type to parse Args more easily
    #[derive(Parser)]
    struct CommandParser<T: Args> {
        #[command(flatten)]
        args: T,
    }

    #[test]
    fn parse_numeric_key_args() {
        assert_eq!(table_key::<Headers>("123").unwrap(), 123);
        assert_eq!(
            table_key::<HashedAccounts>(
                "\"0x0ac361fe774b78f8fc4e86c1916930d150865c3fc2e21dca2e58833557608bac\""
            )
            .unwrap(),
            B256::from_str("0x0ac361fe774b78f8fc4e86c1916930d150865c3fc2e21dca2e58833557608bac")
                .unwrap()
        );
    }

    #[test]
    fn parse_string_key_args() {
        assert_eq!(
            table_key::<StageCheckpoints>("\"MerkleExecution\"").unwrap(),
            "MerkleExecution"
        );
    }

    #[test]
    fn parse_json_key_args() {
        assert_eq!(
            table_key::<StoragesHistory>(r#"{ "address": "0x01957911244e546ce519fbac6f798958fafadb41", "sharded_key": { "key": "0x0000000000000000000000000000000000000000000000000000000000000003", "highest_block_number": 18446744073709551615 } }"#).unwrap(),
            StorageShardedKey::new(
                address!("0x01957911244e546ce519fbac6f798958fafadb41"),
                B256::from_str(
                    "0x0000000000000000000000000000000000000000000000000000000000000003"
                )
                .unwrap(),
                18446744073709551615
            )
        );
    }

    #[test]
    fn parse_json_key_for_account_history() {
        assert_eq!(
            table_key::<AccountsHistory>(r#"{ "key": "0x4448e1273fd5a8bfdb9ed111e96889c960eee145", "highest_block_number": 18446744073709551615 }"#).unwrap(),
            ShardedKey::new(
                address!("0x4448e1273fd5a8bfdb9ed111e96889c960eee145"),
                18446744073709551615
            )
        );
    }
}
