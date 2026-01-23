use super::tui::DbListTUI;
use alloy_primitives::hex;
use clap::Parser;
use eyre::WrapErr;
use reth_chainspec::EthereumHardforks;
use reth_db::{transaction::DbTx, DatabaseEnv};
use reth_db_api::{database::Database, table::Table, RawValue, TableViewer, Tables};
use reth_db_common::{DbTool, ListFilter};
use reth_node_builder::{NodeTypes, NodeTypesWithDBAdapter};
use std::{cell::RefCell, sync::Arc};
use tracing::error;

#[cfg(all(unix, feature = "edge"))]
mod rocksdb;

#[derive(Parser, Debug)]
pub struct Command {
    #[command(subcommand)]
    subcommand: Subcommand,
}

#[derive(clap::Subcommand, Debug)]
enum Subcommand {
    Mdbx {
        table: Tables,
        #[arg(long, short, default_value_t = 0)]
        skip: usize,
        #[arg(long, short, default_value_t = false)]
        reverse: bool,
        #[arg(long, short, default_value_t = 5)]
        len: usize,
        #[arg(long)]
        search: Option<String>,
        #[arg(long, default_value_t = 0)]
        min_row_size: usize,
        #[arg(long, default_value_t = 0)]
        min_key_size: usize,
        #[arg(long, default_value_t = 0)]
        min_value_size: usize,
        #[arg(long, short)]
        count: bool,
        #[arg(long, short)]
        json: bool,
        #[arg(long)]
        raw: bool,
    },
    #[cfg(all(unix, feature = "edge"))]
    Rocksdb {
        #[arg(value_enum)]
        table: rocksdb::RocksDbTable,
        #[arg(long, short, default_value_t = 0)]
        skip: usize,
        #[arg(long, short, default_value_t = false)]
        reverse: bool,
        #[arg(long, short, default_value_t = 5)]
        len: usize,
        #[arg(long)]
        search: Option<String>,
        #[arg(long, default_value_t = 0)]
        min_row_size: usize,
        #[arg(long, default_value_t = 0)]
        min_key_size: usize,
        #[arg(long, default_value_t = 0)]
        min_value_size: usize,
        #[arg(long, short)]
        count: bool,
        #[arg(long, short)]
        json: bool,
        #[arg(long)]
        raw: bool,
    },
}

impl Command {
    pub fn execute<N: NodeTypes<ChainSpec: EthereumHardforks>>(
        self,
        tool: &DbTool<NodeTypesWithDBAdapter<N, Arc<DatabaseEnv>>>,
    ) -> eyre::Result<()> {
        match self.subcommand {
            Subcommand::Mdbx {
                table,
                skip,
                reverse,
                len,
                search,
                min_row_size,
                min_key_size,
                min_value_size,
                count,
                json,
                raw,
            } => {
                let args = MdbxArgs {
                    table,
                    skip,
                    reverse,
                    len,
                    search,
                    min_row_size,
                    min_key_size,
                    min_value_size,
                    count,
                    json,
                    raw,
                };
                args.table.view(&ListTableViewer { tool, args: &args })
            }
            #[cfg(all(unix, feature = "edge"))]
            Subcommand::Rocksdb {
                table,
                skip,
                reverse,
                len,
                search,
                min_row_size,
                min_key_size,
                min_value_size,
                count,
                json,
                raw,
            } => {
                let args = rocksdb::RocksDbArgs {
                    skip,
                    reverse,
                    len,
                    search,
                    min_row_size,
                    min_key_size,
                    min_value_size,
                    count,
                    json,
                    raw,
                };
                rocksdb::list_rocksdb(tool, table, args)
            }
        }
    }
}

#[derive(Debug)]
struct MdbxArgs {
    table: Tables,
    skip: usize,
    reverse: bool,
    len: usize,
    search: Option<String>,
    min_row_size: usize,
    min_key_size: usize,
    min_value_size: usize,
    count: bool,
    json: bool,
    raw: bool,
}

impl MdbxArgs {
    fn list_filter(&self) -> eyre::Result<ListFilter> {
        let search = match self.search.as_deref() {
            Some(search) => {
                if let Some(search) = search.strip_prefix("0x") {
                    hex::decode(search).wrap_err(
                        "Invalid hex content after 0x prefix in --search (expected valid hex like 0xdeadbeef).",
                    )?
                } else {
                    search.as_bytes().to_vec()
                }
            }
            None => Vec::new(),
        };

        Ok(ListFilter {
            skip: self.skip,
            len: self.len,
            search,
            min_row_size: self.min_row_size,
            min_key_size: self.min_key_size,
            min_value_size: self.min_value_size,
            reverse: self.reverse,
            only_count: self.count,
        })
    }
}

struct ListTableViewer<'a, N: NodeTypes> {
    tool: &'a DbTool<NodeTypesWithDBAdapter<N, Arc<DatabaseEnv>>>,
    args: &'a MdbxArgs,
}

impl<N: NodeTypes> TableViewer<()> for ListTableViewer<'_, N> {
    type Error = eyre::Report;

    fn view<T: Table>(&self) -> Result<(), Self::Error> {
        self.tool.provider_factory.db_ref().view(|tx| {
            tx.disable_long_read_transaction_safety();

            let table_db =
                tx.inner.open_db(Some(self.args.table.name())).wrap_err("Could not open db.")?;
            let stats = tx.inner.db_stat(table_db.dbi()).wrap_err(format!(
                "Could not find table: {}",
                self.args.table.name()
            ))?;
            let total_entries = stats.entries();
            let final_entry_idx = total_entries.saturating_sub(1);
            if self.args.skip > final_entry_idx {
                error!(
                    target: "reth::cli",
                    "Start index {start} is greater than the final entry index ({final_entry_idx}) in the table {table}",
                    start = self.args.skip,
                    final_entry_idx = final_entry_idx,
                    table = self.args.table.name()
                );
                return Ok(());
            }

            let list_filter = self.args.list_filter()?;

            if self.args.json || self.args.count {
                let (list, count) = self.tool.list::<T>(&list_filter)?;

                if self.args.count {
                    println!("{count} entries found.")
                } else if self.args.raw {
                    let list = list
                        .into_iter()
                        .map(|row| (row.0, RawValue::new(row.1).into_value()))
                        .collect::<Vec<_>>();
                    println!("{}", serde_json::to_string_pretty(&list)?);
                } else {
                    println!("{}", serde_json::to_string_pretty(&list)?);
                }
                Ok(())
            } else {
                let list_filter = RefCell::new(list_filter);
                DbListTUI::<_, T>::new(
                    |skip, len| {
                        list_filter.borrow_mut().update_page(skip, len);
                        self.tool.list::<T>(&list_filter.borrow()).unwrap().0
                    },
                    self.args.skip,
                    self.args.len,
                    total_entries,
                    self.args.raw,
                )
                .run()
            }
        })??;

        Ok(())
    }
}
