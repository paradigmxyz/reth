use crate::db::get::{maybe_json_value_parser, table_key};
use ahash::RandomState;
use clap::Parser;
use reth_chainspec::EthereumHardforks;
use reth_db::{DatabaseEnv, RawKey, RawTable, RawValue, TableViewer, Tables};
use reth_db_api::{cursor::DbCursorRO, table::Table, transaction::DbTx};
use reth_db_common::DbTool;
use reth_node_builder::{NodeTypesWithDB, NodeTypesWithDBAdapter, NodeTypesWithEngine};
use reth_provider::providers::ProviderNodeTypes;
use std::{
    hash::{BuildHasher, Hasher},
    sync::Arc,
    time::{Duration, Instant},
};
use tracing::{info, warn};

#[derive(Parser, Debug)]
/// The arguments for the `reth db checksum` command
pub struct Command {
    /// The table name
    table: Tables,

    /// The start of the range to checksum.
    #[arg(long, value_parser = maybe_json_value_parser)]
    start_key: Option<String>,

    /// The end of the range to checksum.
    #[arg(long, value_parser = maybe_json_value_parser)]
    end_key: Option<String>,

    /// The maximum number of records that are queried and used to compute the
    /// checksum.
    #[arg(long)]
    limit: Option<usize>,
}

impl Command {
    /// Execute `db checksum` command
    pub fn execute<N: NodeTypesWithEngine<ChainSpec: EthereumHardforks>>(
        self,
        tool: &DbTool<NodeTypesWithDBAdapter<N, Arc<DatabaseEnv>>>,
    ) -> eyre::Result<()> {
        warn!("This command should be run without the node running!");
        self.table.view(&ChecksumViewer {
            tool,
            start_key: self.start_key,
            end_key: self.end_key,
            limit: self.limit,
        })?;
        Ok(())
    }
}

pub(crate) struct ChecksumViewer<'a, N: NodeTypesWithDB> {
    tool: &'a DbTool<N>,
    start_key: Option<String>,
    end_key: Option<String>,
    limit: Option<usize>,
}

impl<N: NodeTypesWithDB> ChecksumViewer<'_, N> {
    pub(crate) const fn new(tool: &'_ DbTool<N>) -> ChecksumViewer<'_, N> {
        ChecksumViewer { tool, start_key: None, end_key: None, limit: None }
    }
}

impl<N: ProviderNodeTypes> TableViewer<(u64, Duration)> for ChecksumViewer<'_, N> {
    type Error = eyre::Report;

    fn view<T: Table>(&self) -> Result<(u64, Duration), Self::Error> {
        let provider =
            self.tool.provider_factory.provider()?.disable_long_read_transaction_safety();
        let tx = provider.tx_ref();
        info!(
            "Start computing checksum, start={:?}, end={:?}, limit={:?}",
            self.start_key, self.end_key, self.limit
        );

        let mut cursor = tx.cursor_read::<RawTable<T>>()?;
        let walker = match (self.start_key.as_deref(), self.end_key.as_deref()) {
            (Some(start), Some(end)) => {
                let start_key = table_key::<T>(start).map(RawKey::<T::Key>::new)?;
                let end_key = table_key::<T>(end).map(RawKey::<T::Key>::new)?;
                cursor.walk_range(start_key..=end_key)?
            }
            (None, Some(end)) => {
                let end_key = table_key::<T>(end).map(RawKey::<T::Key>::new)?;

                cursor.walk_range(..=end_key)?
            }
            (Some(start), None) => {
                let start_key = table_key::<T>(start).map(RawKey::<T::Key>::new)?;
                cursor.walk_range(start_key..)?
            }
            (None, None) => cursor.walk_range(..)?,
        };

        let start_time = Instant::now();
        let mut hasher = RandomState::with_seeds(1, 2, 3, 4).build_hasher();
        let mut total = 0;

        let limit = self.limit.unwrap_or(usize::MAX);
        let mut enumerate_start_key = None;
        let mut enumerate_end_key = None;
        for (index, entry) in walker.enumerate() {
            let (k, v): (RawKey<T::Key>, RawValue<T::Value>) = entry?;

            if index % 100_000 == 0 {
                info!("Hashed {index} entries.");
            }

            hasher.write(k.raw_key());
            hasher.write(v.raw_value());

            if enumerate_start_key.is_none() {
                enumerate_start_key = Some(k.clone());
            }
            enumerate_end_key = Some(k);

            total = index + 1;
            if total >= limit {
                break
            }
        }

        info!("Hashed {total} entries.");
        if let (Some(s), Some(e)) = (enumerate_start_key, enumerate_end_key) {
            info!("start-key: {}", serde_json::to_string(&s.key()?).unwrap_or_default());
            info!("end-key: {}", serde_json::to_string(&e.key()?).unwrap_or_default());
        }

        let checksum = hasher.finish();
        let elapsed = start_time.elapsed();

        info!("Checksum for table `{}`: {:#x} (elapsed: {:?})", T::NAME, checksum, elapsed);

        Ok((checksum, elapsed))
    }
}
