use crate::utils::DbTool;
use clap::Parser;
use reth_db::{
    database::Database,
    static_file::{ColumnSelectorOne, ColumnSelectorTwo, HeaderMask, ReceiptMask, TransactionMask},
    table::{Decompress, DupSort, Table},
    tables, RawKey, RawTable, Receipts, TableViewer, Transactions,
};
use reth_primitives::{BlockHash, Header, StaticFileSegment};
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

        /// Output bytes instead of human-readable decoded value
        #[arg(long)]
        raw: bool,
    },
}

impl Command {
    /// Execute `db get` command
    pub fn execute<DB: Database>(self, tool: &DbTool<DB>) -> eyre::Result<()> {
        match self.subcommand {
            Subcommand::Mdbx { table, key, subkey, raw } => {
                table.view(&GetValueViewer { tool, key, subkey, raw })?
            }
            Subcommand::StaticFile { segment, key, raw } => {
                let (key, mask): (u64, _) = match segment {
                    StaticFileSegment::Headers => {
                        (table_key::<tables::Headers>(&key)?, <HeaderMask<Header, BlockHash>>::MASK)
                    }
                    StaticFileSegment::Transactions => (
                        table_key::<tables::Transactions>(&key)?,
                        <TransactionMask<<Transactions as Table>::Value>>::MASK,
                    ),
                    StaticFileSegment::Receipts => (
                        table_key::<tables::Receipts>(&key)?,
                        <ReceiptMask<<Receipts as Table>::Value>>::MASK,
                    ),
                };

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
                            println!("{content:?}");
                        } else {
                            match segment {
                                StaticFileSegment::Headers => {
                                    let header = Header::decompress(content[0].as_slice())?;
                                    let block_hash = BlockHash::decompress(content[1].as_slice())?;
                                    println!(
                                        "{}\n{}",
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
fn table_key<T: Table>(key: &str) -> Result<T::Key, eyre::Error> {
    serde_json::from_str::<T::Key>(key).map_err(|e| eyre::eyre!(e))
}

/// Get an instance of subkey for given dupsort table
fn table_subkey<T: DupSort>(subkey: &Option<String>) -> Result<T::SubKey, eyre::Error> {
    serde_json::from_str::<T::SubKey>(&subkey.clone().unwrap_or_default())
        .map_err(|e| eyre::eyre!(e))
}

struct GetValueViewer<'a, DB: Database> {
    tool: &'a DbTool<DB>,
    key: String,
    subkey: Option<String>,
    raw: bool,
}

impl<DB: Database> TableViewer<()> for GetValueViewer<'_, DB> {
    type Error = eyre::Report;

    fn view<T: Table>(&self) -> Result<(), Self::Error> {
        let key = table_key::<T>(&self.key)?;

        let content = if self.raw {
            self.tool
                .get::<RawTable<T>>(RawKey::from(key))?
                .map(|content| format!("{:?}", content.raw_value()))
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

        Ok(())
    }

    fn view_dupsort<T: DupSort>(&self) -> Result<(), Self::Error> {
        // get a key for given table
        let key = table_key::<T>(&self.key)?;

        // process dupsort table
        let subkey = table_subkey::<T>(&self.subkey)?;

        match self.tool.get_dup::<T>(key, subkey)? {
            Some(content) => {
                println!("{}", serde_json::to_string_pretty(&content)?);
            }
            None => {
                error!(target: "reth::cli", "No content for the given table subkey.");
            }
        };
        Ok(())
    }
}

/// Map the user input value to json
fn maybe_json_value_parser(value: &str) -> Result<String, eyre::Error> {
    if serde_json::from_str::<serde::de::IgnoredAny>(value).is_ok() {
        Ok(value.to_string())
    } else {
        serde_json::to_string(&value).map_err(|e| eyre::eyre!(e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{Args, Parser};
    use reth_db::{
        models::{storage_sharded_key::StorageShardedKey, ShardedKey},
        AccountsHistory, HashedAccounts, Headers, StageCheckpoints, StoragesHistory,
    };
    use reth_primitives::{Address, B256};
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
                Address::from_str("0x01957911244e546ce519fbac6f798958fafadb41").unwrap(),
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
                Address::from_str("0x4448e1273fd5a8bfdb9ed111e96889c960eee145").unwrap(),
                18446744073709551615
            )
        );
    }
}
