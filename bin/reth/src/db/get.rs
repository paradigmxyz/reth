use crate::utils::DbTool;
use clap::Parser;
use eyre::WrapErr;
use reth_db::{database::Database, table::Table, tables};
use serde::Deserialize;
use tracing::error;

/// The arguments for the `reth db get` command
#[derive(Parser, Debug)]
pub struct Command {
    /// The table name
    ///
    /// NOTE: The dupsort tables are not supported now.
    #[arg()]
    pub table: String, // TODO: Convert to enum

    /// The key to get content for
    #[arg(value_parser = maybe_json_value_parser)]
    pub key: String,
}

impl Command {
    /// Execute `db get` command
    pub fn execute<DB: Database>(self, mut tool: DbTool<'_, DB>) -> eyre::Result<()> {
        macro_rules! table_get {
            ([$($table:ident),*]) => {
                match self.table.as_str() {
                    $(stringify!($table) => {
                        let table_key = self.table_key::<tables::$table>().wrap_err("Could not parse the given table key.")?;

                        match tool.get::<tables::$table>(table_key)? {
                            Some(content) => {
                                println!("{}", serde_json::to_string_pretty(&content)?);
                            }
                            None => {
                                error!(target: "reth::cli", "No content for the given table key.");
                            },
                        };
                        return Ok(());
                    },)*
                    _ => {
                        error!(target: "reth::cli", "Unknown or unsupported table.");
                        return Ok(());
                    }
                }
            }
        }

        table_get!([
            CanonicalHeaders,
            HeaderTD,
            HeaderNumbers,
            Headers,
            BlockBodyIndices,
            BlockOmmers,
            BlockWithdrawals,
            TransactionBlock,
            Transactions,
            TxHashNumber,
            Receipts,
            PlainAccountState,
            Bytecodes,
            AccountHistory,
            StorageHistory,
            HashedAccount,
            AccountsTrie,
            TxSenders,
            SyncStage,
            SyncStageProgress
        ]);
    }

    /// Get an instance of key for given table
    fn table_key<T: Table>(&self) -> Result<T::Key, eyre::Error>
    where
        for<'a> T::Key: Deserialize<'a>,
    {
        assert_eq!(T::NAME, self.table);

        serde_json::from_str::<T::Key>(&self.key).map_err(|e| eyre::eyre!(e))
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
        AccountHistory, HashedAccount, Headers, StorageHistory, SyncStage,
    };
    use reth_primitives::{H160, H256};
    use std::str::FromStr;

    /// A helper type to parse Args more easily
    #[derive(Parser)]
    struct CommandParser<T: Args> {
        #[clap(flatten)]
        args: T,
    }

    #[test]
    fn parse_numeric_key_args() {
        let args = CommandParser::<Command>::parse_from(["reth", "Headers", "123"]).args;
        assert_eq!(args.table_key::<Headers>().unwrap(), 123);

        let args = CommandParser::<Command>::parse_from([
            "reth",
            "HashedAccount",
            "0x0ac361fe774b78f8fc4e86c1916930d150865c3fc2e21dca2e58833557608bac",
        ])
        .args;
        assert_eq!(
            args.table_key::<HashedAccount>().unwrap(),
            H256::from_str("0x0ac361fe774b78f8fc4e86c1916930d150865c3fc2e21dca2e58833557608bac")
                .unwrap()
        );
    }

    #[test]
    fn parse_string_key_args() {
        let args =
            CommandParser::<Command>::parse_from(["reth", "SyncStage", "MerkleExecution"]).args;
        assert_eq!(args.table_key::<SyncStage>().unwrap(), "MerkleExecution");
    }

    #[test]
    fn parse_json_key_args() {
        let args = CommandParser::<Command>::parse_from(["reth", "StorageHistory", r#"{ "address": "0x01957911244e546ce519fbac6f798958fafadb41", "sharded_key": { "key": "0x0000000000000000000000000000000000000000000000000000000000000003", "highest_block_number": 18446744073709551615 } }"#]).args;
        assert_eq!(
            args.table_key::<StorageHistory>().unwrap(),
            StorageShardedKey::new(
                H160::from_str("0x01957911244e546ce519fbac6f798958fafadb41").unwrap(),
                H256::from_str(
                    "0x0000000000000000000000000000000000000000000000000000000000000003"
                )
                .unwrap(),
                18446744073709551615
            )
        );
    }

    #[test]
    fn parse_json_key_for_account_history() {
        let args = CommandParser::<Command>::parse_from(["reth", "AccountHistory", r#"{ "key": "0x4448e1273fd5a8bfdb9ed111e96889c960eee145", "highest_block_number": 18446744073709551615 }"#]).args;
        assert_eq!(
            args.table_key::<AccountHistory>().unwrap(),
            ShardedKey::new(
                H160::from_str("0x4448e1273fd5a8bfdb9ed111e96889c960eee145").unwrap(),
                18446744073709551615
            )
        );
    }
}
