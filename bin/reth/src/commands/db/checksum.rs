use crate::utils::DbTool;
use blake3::Hasher;
use clap::Parser;
use reth_db::{
    cursor::DbCursorRO, database::Database, table::Table, transaction::DbTx, DatabaseEnv, RawKey,
    RawTable, RawValue, TableViewer, Tables,
};
use tracing::{info, warn};

#[derive(Parser, Debug)]
/// The arguments for the `reth db checksum` command
pub struct Command {
    /// The table name
    table: Tables,
}

impl Command {
    /// Execute `db checksum` command
    pub fn execute(self, tool: &DbTool<DatabaseEnv>) -> eyre::Result<()> {
        self.table.view(&ChecksumViewer { tool })
    }
}

struct ChecksumViewer<'a, DB: Database> {
    tool: &'a DbTool<DB>,
}

impl<DB: Database> TableViewer<()> for ChecksumViewer<'_, DB> {
    type Error = eyre::Report;

    fn view<T: Table>(&self) -> Result<(), Self::Error> {
        warn!("This command should be run without the node running!");

        let provider = self.tool.provider_factory.provider()?.disable_long_read_transaction_safety();
        let tx = provider.tx_ref();

        let mut cursor = tx.cursor_read::<RawTable<T>>()?;
        let walker = cursor.walk(None)?;

        let mut hasher = Hasher::new();
        for (index, entry) in walker.enumerate() {
            let (k, v): (RawKey<T::Key>, RawValue<T::Value>) = entry?;

            if index % 100_000 == 0 {
                info!("Hashed {index} entries.");
            }

            hasher.update(k.raw_key());
            hasher.update(v.raw_value());
        }

        info!("{:#}", hasher.finalize());

        Ok(())
    }
}
