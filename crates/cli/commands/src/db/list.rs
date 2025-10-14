use alloy_primitives::hex;
use clap::Parser;
use reth_chainspec::EthereumHardforks;
use reth_db::DatabaseEnv;
use reth_db_api::{
    cursor::DbCursorRO, database::Database, table::Table, transaction::DbTx, DatabaseError,
    TableViewer, Tables,
};
use reth_db_common::{DbTool, ListFilter};
use reth_node_builder::{NodeTypes, NodeTypesWithDBAdapter};
use std::sync::Arc;

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
    pub fn execute<N: NodeTypes<ChainSpec: EthereumHardforks>>(
        self,
        tool: &DbTool<NodeTypesWithDBAdapter<N, Arc<DatabaseEnv>>>,
    ) -> eyre::Result<()> {
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

struct ListTableViewer<'a, N: NodeTypes> {
    tool: &'a DbTool<NodeTypesWithDBAdapter<N, Arc<DatabaseEnv>>>,
    args: &'a Command,
}

impl<N: NodeTypes> TableViewer<()> for ListTableViewer<'_, N> {
    type Error = eyre::Report;

    fn view<T: Table>(&self) -> Result<(), Self::Error> {
        let _ = self.tool.provider_factory.db_ref().view(|tx| {
            let mut cursor = tx.cursor_read::<T>()?;
            let mut entries = Vec::new();
            let mut current_idx = 0;

            if let Some((key, value)) = cursor.first()? {
                if current_idx >= self.args.skip {
                    entries.push((key, value));
                }
                current_idx += 1;
                while entries.len() < self.args.len as usize {
                    if let Some((key, value)) = cursor.next()? {
                        if current_idx >= self.args.skip {
                            entries.push((key, value));
                        }
                        current_idx += 1;
                    } else {
                        break;
                    }
                }
            }

            for (key, value) in entries {
                println!("{:?}: {:?}", key, value);
            }

            Ok::<(), DatabaseError>(())
        })?;

        Ok(())
    }
}
