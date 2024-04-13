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
    /// start of range
    start: Option<String>,
    /// end of range
    end: Option<String>,
}

impl Command {
    /// Execute `db checksum` command
    pub fn execute(self, tool: &DbTool<DatabaseEnv>) -> eyre::Result<()> {
        warn!("This command should be run without the node running!");
        self.table.view(&ChecksumViewer { tool, start: self.start, end: self.end })
    }
}

pub(crate) struct ChecksumViewer<'a, DB: Database> {
    tool: &'a DbTool<DB>,
    start: Option<String>,
    end: Option<String>,
}

impl<DB: Database> ChecksumViewer<'_, DB> {
    pub(crate) fn new(tool: &'_ DbTool<DB>) -> ChecksumViewer<'_, DB> {
        ChecksumViewer { tool, start: None, end: None }
    }

    pub(crate) fn get_checksum<T: Table>(&self) -> Result<(u64, Duration), eyre::Report>
    where
        <T as Table>::Key: std::str::FromStr,
    {
        let provider =
            self.tool.provider_factory.provider()?.disable_long_read_transaction_safety();
        let tx = provider.tx_ref();

        let mut cursor = tx.cursor_read::<RawTable<T>>()?;
        let walker = match (self.start.as_deref(), self.end.as_deref()) {
            (Some(start), Some(end)) => {
                let start_key =
                    start.parse::<T::Key>().map_err(|_| eyre::eyre!("Invalid start key"))?;
                let end_key = end.parse::<T::Key>().map_err(|_| eyre::eyre!("Invalid end key"))?;

                RawKey::new(start_key)..=RawKey::new(end_key)
                cursor.walk_range(start_key..=end_key)?
            }
            (None, Some(end)) => {
                let end_key =
                    end.parse::<RawKey<T::Key>>().map_err(|_| eyre::eyre!("Invalid end key"))?;
                cursor.walk_range(..=end_key)?
            }
            (_, None) => cursor.walk_range(..)?,
        };

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

    fn view<T: Table>(&self) -> Result<(), Self::Error>
    where
        <T as Table>::Key: std::str::FromStr,
    {
        let (checksum, elapsed) = self.get_checksum::<T>()?;
        info!("Checksum for table `{}`: {:#x} (elapsed: {:?})", T::NAME, checksum, elapsed);

        Ok(())
    }
}
