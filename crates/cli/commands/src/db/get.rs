use alloy_primitives::{hex, BlockHash};
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
    table::{Compress, Decompress, DupSort, Table},
    tables,
    transaction::DbTx,
    RawKey, RawTable, Receipts, TableViewer, Transactions,
};
use reth_db_common::DbTool;
use reth_node_api::{HeaderTy, ReceiptTy, TxTy};
use reth_node_builder::NodeTypesWithDB;
use reth_primitives_traits::ValueWithSubKey;
use reth_provider::{providers::ProviderNodeTypes, ChangeSetReader, StaticFileProviderFactory};
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
}

impl Command {
    /// Execute `db get` command
    pub fn execute<N: ProviderNodeTypes>(self, tool: &DbTool<N>) -> eyre::Result<()> {
        match self.subcommand {
            Subcommand::Mdbx { table, key, subkey, end_key, end_subkey, raw } => {
                table.view(&GetValueViewer { tool, key, subkey, end_key, end_subkey, raw })?
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
                            println!("{}", serde_json::to_string_pretty(&entry)?);
                        } else {
                            error!(target: "reth::cli", "No content for the given table key.");
                        }
                        return Ok(());
                    }

                    let changesets = provider.storage_changeset(key.block_number())?;
                    println!("{}", serde_json::to_string_pretty(&changesets)?);
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
