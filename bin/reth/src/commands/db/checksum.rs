use crate::utils::DbTool;
use ahash::AHasher;
use clap::Parser;
use reth_db::{
    cursor::DbCursorRO, database::Database, table::Table, transaction::DbTx, DatabaseEnv, RawKey,
    RawTable, RawValue, TableViewer, Tables,
};
use std::{
    hash::Hasher,
    time::{Duration, Instant},
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
        warn!("This command should be run without the node running!");
        self.table.view(&ChecksumViewer { tool })
    }
}

pub(crate) struct ChecksumViewer<'a, DB: Database> {
    tool: &'a DbTool<DB>,
}

impl<DB: Database> ChecksumViewer<'_, DB> {
    pub(crate) fn new(tool: &'_ DbTool<DB>) -> ChecksumViewer<'_, DB> {
        ChecksumViewer { tool }
    }

    pub(crate) fn get_checksum<T: Table>(&self) -> Result<(u64, Duration), eyre::Report> {
        let provider =
            self.tool.provider_factory.provider()?.disable_long_read_transaction_safety();
        let tx = provider.tx_ref();

        let mut cursor = tx.cursor_read::<RawTable<T>>()?;
        let walker = cursor.walk(None)?;

        let start_time = Instant::now();
        let mut hasher = AHasher::default();
        for (index, entry) in walker.enumerate() {
            let (k, v): (RawKey<T::Key>, RawValue<T::Value>) = entry?;

            if index % 100_000 == 0 {
                info!("Hashed {index} entries.");
            }

            hasher.write(k.raw_key());
            hasher.write(v.raw_value());
        }

        let checksum = hasher.finish();
        let elapsed = start_time.elapsed();

        Ok((checksum, elapsed))
    }
}

impl<DB: Database> TableViewer<()> for ChecksumViewer<'_, DB> {
    type Error = eyre::Report;

    fn view<T: Table>(&self) -> Result<(), Self::Error> {
        let (checksum, elapsed) = self.get_checksum::<T>()?;
        info!("Checksum for table `{}`: {:#x} (elapsed: {:?})", T::NAME, checksum, elapsed);

        Ok(())
    }
}
