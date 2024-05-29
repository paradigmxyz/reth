use super::tui::DbListTUI;
use crate::utils::{DbTool, ListFilter};
use clap::Parser;
use eyre::WrapErr;
use reth_db::{database::Database, table::Table, DatabaseEnv, RawValue, TableViewer, Tables};
use reth_primitives::hex;
use std::cell::RefCell;
use tracing::error;

#[derive(Parser, Debug)]
/// The arguments for the `reth db list` command
pub struct Command {
    /// The table name
    table: Tables,
    /// Skip first N entries
    #[arg(long, short, default_value_t = 0)]
    skip: usize,
    /// Reverse the order of the entries. If enabled last table entries are read.
    #[arg(long, short, default_value_t = false)]
    reverse: bool,
    /// How many items to take from the walker
    #[arg(long, short, default_value_t = 5)]
    len: usize,
    /// Search parameter for both keys and values. Prefix it with `0x` to search for binary data,
    /// and text otherwise.
    ///
    /// ATTENTION! For compressed tables (`Transactions` and `Receipts`), there might be
    /// missing results since the search uses the raw uncompressed value from the database.
    #[arg(long)]
    search: Option<String>,
    /// Minimum size of row in bytes
    #[arg(long, default_value_t = 0)]
    min_row_size: usize,
    /// Minimum size of key in bytes
    #[arg(long, default_value_t = 0)]
    min_key_size: usize,
    /// Minimum size of value in bytes
    #[arg(long, default_value_t = 0)]
    min_value_size: usize,
    /// Returns the number of rows found.
    #[arg(long, short)]
    count: bool,
    /// Dump as JSON instead of using TUI.
    #[arg(long, short)]
    json: bool,
    /// Output bytes instead of human-readable decoded value
    #[arg(long)]
    raw: bool,
}

impl Command {
    /// Execute `db list` command
    pub fn execute(self, tool: &DbTool<DatabaseEnv>) -> eyre::Result<()> {
        self.table.view(&ListTableViewer { tool, args: &self })
    }

    /// Generate [`ListFilter`] from command.
    pub fn list_filter(&self) -> ListFilter {
        let search = self
            .search
            .as_ref()
            .map(|search| {
                if let Some(search) = search.strip_prefix("0x") {
                    return hex::decode(search).unwrap()
                }
                search.as_bytes().to_vec()
            })
            .unwrap_or_default();

        ListFilter {
            skip: self.skip,
            len: self.len,
            search,
            min_row_size: self.min_row_size,
            min_key_size: self.min_key_size,
            min_value_size: self.min_value_size,
            reverse: self.reverse,
            only_count: self.count,
        }
    }
}

struct ListTableViewer<'a> {
    tool: &'a DbTool<DatabaseEnv>,
    args: &'a Command,
}

impl TableViewer<()> for ListTableViewer<'_> {
    type Error = eyre::Report;

    fn view<T: Table>(&self) -> Result<(), Self::Error> {
        self.tool.provider_factory.db_ref().view(|tx| {
            let table_db = tx.inner.open_db(Some(self.args.table.name())).wrap_err("Could not open db.")?;
            let stats = tx.inner.db_stat(&table_db).wrap_err(format!("Could not find table: {}", stringify!($table)))?;
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
                return Ok(())
            }


            let list_filter = self.args.list_filter();

            if self.args.json || self.args.count {
                let (list, count) = self.tool.list::<T>(&list_filter)?;

                if self.args.count {
                    println!("{count} entries found.")
                } else if self.args.raw {
                    let list = list.into_iter().map(|row| (row.0, RawValue::new(row.1).into_value())).collect::<Vec<_>>();
                    println!("{}", serde_json::to_string_pretty(&list)?);
                } else {
                    println!("{}", serde_json::to_string_pretty(&list)?);
                }
                Ok(())
            } else {
                let list_filter = RefCell::new(list_filter);
                DbListTUI::<_, T>::new(|skip, len| {
                    list_filter.borrow_mut().update_page(skip, len);
                    self.tool.list::<T>(&list_filter.borrow()).unwrap().0
                }, self.args.skip, self.args.len, total_entries, self.args.raw).run()
            }
        })??;

        Ok(())
    }
}
