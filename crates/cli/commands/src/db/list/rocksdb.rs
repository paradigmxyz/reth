use crate::common::CliNodeTypes;
use alloy_primitives::hex;
use clap::ValueEnum;
use eyre::WrapErr;
use reth_chainspec::EthereumHardforks;
use reth_db::{tables, DatabaseEnv};
use reth_db_api::table::{Decode, Decompress, Table};
use reth_db_common::DbTool;
use reth_node_builder::NodeTypesWithDBAdapter;
use reth_provider::RocksDBProviderFactory;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum RocksDbTable {
    TransactionHashNumbers,
    AccountsHistory,
    StoragesHistory,
}

impl RocksDbTable {
    pub const fn name(&self) -> &'static str {
        match self {
            Self::TransactionHashNumbers => tables::TransactionHashNumbers::NAME,
            Self::AccountsHistory => tables::AccountsHistory::NAME,
            Self::StoragesHistory => tables::StoragesHistory::NAME,
        }
    }
}

#[derive(Debug)]
pub struct RocksDbArgs {
    pub skip: usize,
    pub reverse: bool,
    pub len: usize,
    pub search: Option<String>,
    pub min_row_size: usize,
    pub min_key_size: usize,
    pub min_value_size: usize,
    pub count: bool,
    pub json: bool,
    pub raw: bool,
}

impl RocksDbArgs {
    fn parse_search(&self) -> eyre::Result<Vec<u8>> {
        match self.search.as_deref() {
            Some(search) => {
                if let Some(search) = search.strip_prefix("0x") {
                    hex::decode(search).wrap_err(
                        "Invalid hex content after 0x prefix in --search (expected valid hex like 0xdeadbeef).",
                    )
                } else {
                    Ok(search.as_bytes().to_vec())
                }
            }
            None => Ok(Vec::new()),
        }
    }
}

pub fn list_rocksdb<N: CliNodeTypes<ChainSpec: EthereumHardforks>>(
    tool: &DbTool<NodeTypesWithDBAdapter<N, Arc<DatabaseEnv>>>,
    table: RocksDbTable,
    args: RocksDbArgs,
) -> eyre::Result<()> {
    match table {
        RocksDbTable::TransactionHashNumbers => {
            list_table::<tables::TransactionHashNumbers>(tool, args)
        }
        RocksDbTable::AccountsHistory => list_table::<tables::AccountsHistory>(tool, args),
        RocksDbTable::StoragesHistory => list_table::<tables::StoragesHistory>(tool, args),
    }
}

fn list_table<T: Table>(
    tool: &DbTool<impl CliNodeTypes<ChainSpec: EthereumHardforks>>,
    args: RocksDbArgs,
) -> eyre::Result<()>
where
    T::Key: serde::Serialize,
    T::Value: serde::Serialize,
{
    let rocksdb = tool.provider_factory.rocksdb_provider();
    let search = args.parse_search()?;
    let mut entries = Vec::new();

    if args.reverse {
        let mut all = collect_entries::<T>(&rocksdb, &args, &search)?;
        let total = all.len();
        let start = total.saturating_sub(args.skip + args.len);
        let end = total.saturating_sub(args.skip);
        entries = all.drain(start..end).rev().collect();
    } else {
        let mut skipped = 0;
        for entry in rocksdb.raw_iter::<T>()? {
            let (key_bytes, value_bytes) = entry?;

            if !matches_filters(&key_bytes, &value_bytes, &args, &search) {
                continue;
            }

            if skipped < args.skip {
                skipped += 1;
                continue;
            }

            entries.push((T::Key::decode(&key_bytes)?, T::Value::decompress(&value_bytes)?));

            if entries.len() >= args.len {
                break;
            }
        }
    }

    if args.count {
        println!("{} entries found.", entries.len());
    } else {
        println!("{}", serde_json::to_string_pretty(&entries)?);
    }

    Ok(())
}

fn collect_entries<T: Table>(
    rocksdb: &reth_provider::providers::RocksDBProvider,
    args: &RocksDbArgs,
    search: &[u8],
) -> eyre::Result<Vec<(T::Key, T::Value)>> {
    let mut entries = Vec::new();
    for entry in rocksdb.raw_iter::<T>()? {
        let (key_bytes, value_bytes) = entry?;
        if matches_filters(&key_bytes, &value_bytes, args, search) {
            entries.push((T::Key::decode(&key_bytes)?, T::Value::decompress(&value_bytes)?));
        }
    }
    Ok(entries)
}

fn matches_filters(key: &[u8], value: &[u8], args: &RocksDbArgs, search: &[u8]) -> bool {
    let row_size = key.len() + value.len();
    if args.min_row_size > 0 && row_size < args.min_row_size {
        return false;
    }
    if args.min_key_size > 0 && key.len() < args.min_key_size {
        return false;
    }
    if args.min_value_size > 0 && value.len() < args.min_value_size {
        return false;
    }
    if !search.is_empty() {
        let key_match = key.windows(search.len()).any(|w| w == search);
        let value_match = value.windows(search.len()).any(|w| w == search);
        if !key_match && !value_match {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_names() {
        assert_eq!(RocksDbTable::TransactionHashNumbers.name(), "TransactionHashNumbers");
        assert_eq!(RocksDbTable::AccountsHistory.name(), "AccountsHistory");
        assert_eq!(RocksDbTable::StoragesHistory.name(), "StoragesHistory");
    }

    #[test]
    fn parse_hex_search() {
        let args = RocksDbArgs {
            search: Some("0xdeadbeef".into()),
            skip: 0,
            reverse: false,
            len: 10,
            min_row_size: 0,
            min_key_size: 0,
            min_value_size: 0,
            count: false,
            json: false,
            raw: false,
        };
        assert_eq!(args.parse_search().unwrap(), vec![0xde, 0xad, 0xbe, 0xef]);
    }

    #[test]
    fn parse_text_search() {
        let args = RocksDbArgs {
            search: Some("hello".into()),
            skip: 0,
            reverse: false,
            len: 10,
            min_row_size: 0,
            min_key_size: 0,
            min_value_size: 0,
            count: false,
            json: false,
            raw: false,
        };
        assert_eq!(args.parse_search().unwrap(), b"hello");
    }
}
