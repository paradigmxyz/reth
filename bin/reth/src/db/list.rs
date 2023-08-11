use crate::utils::DbTool;
use clap::Parser;

use super::tui::DbListTUI;
use eyre::WrapErr;
use reth_db::{database::Database, table::Table, DatabaseEnvRO, TableType, TableViewer, Tables};
use tracing::error;

const DEFAULT_NUM_ITEMS: &str = "5";

#[derive(Parser, Debug)]
/// The arguments for the `reth db list` command
pub struct Command {
    /// The table name
    table: Tables,
    /// Skip first N entries
    #[arg(long, short, default_value = "0")]
    skip: usize,
    /// Reverse the order of the entries. If enabled last table entries are read.
    #[arg(long, short, default_value = "false")]
    reverse: bool,
    /// How many items to take from the walker
    #[arg(long, short, default_value = DEFAULT_NUM_ITEMS)]
    len: usize,
    /// Search parameter for both keys and values. Prefix it with `0x` to search for binary data,
    /// and text otherwise.
    ///
    /// ATTENTION! For compressed tables (`Transactions` and `Receipts`), there might be
    /// missing results since the search uses the raw uncompressed value from the database.
    #[arg(long)]
    search: Option<String>,
    /// Returns the number of rows found.
    #[arg(long, short)]
    count: bool,
    /// Dump as JSON instead of using TUI.
    #[arg(long, short)]
    json: bool,
}

impl Command {
    /// Execute `db list` command
    pub fn execute(self, tool: &DbTool<'_, DatabaseEnvRO>) -> eyre::Result<()> {
        if self.table.table_type() == TableType::DupSort {
            error!(target: "reth::cli", "Unsupported table.");
        }

        self.table.view(&ListTableViewer { tool, args: &self })?;

        Ok(())
    }
}

struct ListTableViewer<'a> {
    tool: &'a DbTool<'a, DatabaseEnvRO>,
    args: &'a Command,
}

impl TableViewer<()> for ListTableViewer<'_> {
    type Error = eyre::Report;

    fn view<T: Table>(&self) -> Result<(), Self::Error> {
        self.tool.db.view(|tx| {
            let table_db = tx.inner.open_db(Some(self.args.table.name())).wrap_err("Could not open db.")?;
            let stats = tx.inner.db_stat(&table_db).wrap_err(format!("Could not find table: {}", stringify!($table)))?;
            let total_entries = stats.entries();
            if self.args.skip > total_entries - 1 {
                error!(
                    target: "reth::cli",
                    "Start index {start} is greater than the final entry index ({final_entry_idx}) in the table {table}",
                    start = self.args.skip,
                    final_entry_idx = total_entries - 1,
                    table = self.args.table.name()
                );
                return Ok(());
            }

            let search = self.args.search.as_ref()
                .map(|search|
                    if search.starts_with("0x") {
                        hex::decode(search.split_at(2).1).unwrap()
                    } else {
                        search.as_bytes().to_vec()
                    })
                .unwrap_or_default();


            if self.args.json || self.args.count {
                let (list, count) = self.tool.list::<T>(self.args.skip, self.args.len, self.args.reverse, search.clone() , self.args.count)?;

                if self.args.count {
                    println!("{count} entries found.")
                }else {
                    println!("{}", serde_json::to_string_pretty(&list)?);
                }
                Ok(())

            } else {
                DbListTUI::<_, T>::new(|skip, count| {
                    self.tool.list::<T>(skip, count, self.args.reverse, search.clone(), self.args.count).unwrap().0
                }, self.args.skip, self.args.len, total_entries).run()
            }
        })??;

        Ok(())
    }
}
