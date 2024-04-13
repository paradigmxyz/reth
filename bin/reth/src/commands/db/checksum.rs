use crate::{primitives::hex, utils::DbTool};
use ahash::AHasher;
use clap::Parser;
use reth_db::{
    cursor::DbCursorRO,
    database::Database,
    table::{Decode, Table},
    transaction::DbTx,
    DatabaseEnv, RawKey, RawTable, RawValue, TableViewer, Tables,
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
    #[arg(long, verbatim_doc_comment)]
    start: Option<String>,

    /// end of range
    #[arg(long, verbatim_doc_comment)]
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

    pub(crate) fn get_checksum<T: Table>(&self) -> Result<(u64, Duration), eyre::Report> {
        let provider =
            self.tool.provider_factory.provider()?.disable_long_read_transaction_safety();
        let tx = provider.tx_ref();

        let mut cursor = tx.cursor_read::<RawTable<T>>()?;
        let walker = match (self.start.as_deref(), self.end.as_deref()) {
            (Some(start), Some(end)) => {
                info!("start={start} \n end={end}");
                let start = hex::decode(start).map_err(|s| eyre::eyre!(s.to_string()))?;
                let start_key = RawKey::<T::Key>::decode(start)
                    .map_err(|_| eyre::eyre!("Invalid start key"))?;

                let end = hex::decode(end).map_err(|_| eyre::eyre!("Invalid end key"))?;
                let end_key =
                    RawKey::<T::Key>::decode(end).map_err(|_| eyre::eyre!("Invalid end key"))?;

                cursor.walk_range(start_key..=end_key)?
            }
            (None, Some(end)) => {
                info!("start=.. \n end={end}");
                let end = hex::decode(end).map_err(|_| eyre::eyre!("Invalid end key"))?;
                let end_key =
                    RawKey::<T::Key>::decode(end).map_err(|_| eyre::eyre!("Invalid end key"))?;

                cursor.walk_range(..=end_key)?
            }
            (Some(start), None) => {
                info!("start={start} \n end= ");
                let start = hex::decode(start).map_err(|s| eyre::eyre!(s.to_string()))?;
                let start_key =
                    RawKey::<T::Key>::decode(start).map_err(|e| eyre::eyre!(e.to_string()))?;

                cursor.walk_range(start_key..)?
            }
            (None, None) => cursor.walk_range(..)?,
        };

        let start_time = Instant::now();
        let mut hasher = AHasher::default();
        let mut total = 0;
        for (index, entry) in walker.enumerate() {
            let (k, v): (RawKey<T::Key>, RawValue<T::Value>) = entry?;

            if index % 100_000 == 0 {
                info!("Hashed {index} entries.");
            }

            hasher.write(k.raw_key());
            hasher.write(v.raw_value());
            total = index;
        }

        info!("Hashed {total} entries.");

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
