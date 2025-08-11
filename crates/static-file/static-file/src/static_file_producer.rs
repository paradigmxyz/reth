//! Support for producing static files.

use crate::{segments, segments::Segment, StaticFileProducerEvent};
use alloy_primitives::BlockNumber;
use parking_lot::Mutex;
use rayon::prelude::*;
use reth_codecs::Compact;
use reth_db_api::table::Value;
use reth_primitives_traits::NodePrimitives;
use reth_provider::{
    providers::StaticFileWriter, BlockReader, ChainStateBlockReader, DBProvider,
    DatabaseProviderFactory, StageCheckpointReader, StaticFileProviderFactory,
};
use reth_prune_types::PruneModes;
use reth_stages_types::StageId;
use reth_static_file_types::{HighestStaticFiles, StaticFileTargets};
use reth_storage_errors::provider::ProviderResult;
use reth_tokio_util::{EventSender, EventStream};
use std::{
    ops::{Deref, RangeInclusive},
    sync::Arc,
    time::Instant,
};
use tracing::{debug, trace};

/// Result of [`StaticFileProducerInner::run`] execution.
pub type StaticFileProducerResult = ProviderResult<StaticFileTargets>;

/// The [`StaticFileProducer`] instance itself with the result of [`StaticFileProducerInner::run`]
pub type StaticFileProducerWithResult<Provider> =
    (StaticFileProducer<Provider>, StaticFileProducerResult);

/// Static File producer. It's a wrapper around [`StaticFileProducer`] that allows to share it
/// between threads.
#[derive(Debug)]
pub struct StaticFileProducer<Provider>(Arc<Mutex<StaticFileProducerInner<Provider>>>);

impl<Provider> StaticFileProducer<Provider> {
    /// Creates a new [`StaticFileProducer`].
    pub fn new(provider: Provider, prune_modes: PruneModes) -> Self {
        Self(Arc::new(Mutex::new(StaticFileProducerInner::new(provider, prune_modes))))
    }
}

impl<Provider> Clone for StaticFileProducer<Provider> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<Provider> Deref for StaticFileProducer<Provider> {
    type Target = Arc<Mutex<StaticFileProducerInner<Provider>>>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// Static File producer routine. See [`StaticFileProducerInner::run`] for more detailed
/// description.
#[derive(Debug)]
pub struct StaticFileProducerInner<Provider> {
    /// Provider factory
    provider: Provider,
    /// Pruning configuration for every part of the data that can be pruned. Set by user, and
    /// needed in [`StaticFileProducerInner`] to prevent attempting to move prunable data to static
    /// files. See [`StaticFileProducerInner::get_static_file_targets`].
    prune_modes: PruneModes,
    event_sender: EventSender<StaticFileProducerEvent>,
}

impl<Provider> StaticFileProducerInner<Provider> {
    fn new(provider: Provider, prune_modes: PruneModes) -> Self {
        Self { provider, prune_modes, event_sender: Default::default() }
    }
}

impl<Provider> StaticFileProducerInner<Provider>
where
    Provider: StaticFileProviderFactory + DatabaseProviderFactory<Provider: ChainStateBlockReader>,
{
    /// Returns the last finalized block number on disk.
    pub fn last_finalized_block(&self) -> ProviderResult<Option<BlockNumber>> {
        self.provider.database_provider_ro()?.last_finalized_block_number()
    }
}

impl<Provider> StaticFileProducerInner<Provider>
where
    Provider: StaticFileProviderFactory
        + DatabaseProviderFactory<
            Provider: StaticFileProviderFactory<
                Primitives: NodePrimitives<
                    SignedTx: Value + Compact,
                    BlockHeader: Value + Compact,
                    Receipt: Value + Compact,
                >,
            > + StageCheckpointReader
                          + BlockReader,
        >,
{
    /// Listen for events on the `static_file_producer`.
    pub fn events(&self) -> EventStream<StaticFileProducerEvent> {
        self.event_sender.new_listener()
    }

    /// Run the `static_file_producer`.
    ///
    /// For each [Some] target in [`StaticFileTargets`], initializes a corresponding [Segment] and
    /// runs it with the provided block range using [`reth_provider::providers::StaticFileProvider`]
    /// and a read-only database transaction from [`DatabaseProviderFactory`]. All segments are run
    /// in parallel.
    ///
    /// NOTE: it doesn't delete the data from database, and the actual deleting (aka pruning) logic
    /// lives in the `prune` crate.
    pub fn run(&self, targets: StaticFileTargets) -> StaticFileProducerResult {
        // If there are no targets, do not produce any static files and return early
        if !targets.any() {
            return Ok(targets)
        }

        debug_assert!(targets.is_contiguous_to_highest_static_files(
            self.provider.static_file_provider().get_highest_static_files()
        ));

        self.event_sender.notify(StaticFileProducerEvent::Started { targets: targets.clone() });

        debug!(target: "static_file", ?targets, "StaticFileProducer started");
        let start = Instant::now();

        let mut segments =
            Vec::<(Box<dyn Segment<Provider::Provider>>, RangeInclusive<BlockNumber>)>::new();

        if let Some(block_range) = targets.transactions.clone() {
            segments.push((Box::new(segments::Transactions), block_range));
        }
        if let Some(block_range) = targets.headers.clone() {
            segments.push((Box::new(segments::Headers), block_range));
        }
        if let Some(block_range) = targets.receipts.clone() {
            segments.push((Box::new(segments::Receipts), block_range));
        }

        segments.par_iter().try_for_each(|(segment, block_range)| -> ProviderResult<()> {
            debug!(target: "static_file", segment = %segment.segment(), ?block_range, "StaticFileProducer segment");
            let start = Instant::now();

            // Create a new database transaction on every segment to prevent long-lived read-only
            // transactions
            let provider = self.provider.database_provider_ro()?.disable_long_read_transaction_safety();
            segment.copy_to_static_files(provider,  block_range.clone())?;

            let elapsed = start.elapsed(); // TODO(alexey): track in metrics
            debug!(target: "static_file", segment = %segment.segment(), ?block_range, ?elapsed, "Finished StaticFileProducer segment");

            Ok(())
        })?;

        self.provider.static_file_provider().commit()?;
        for (segment, block_range) in segments {
            self.provider
                .static_file_provider()
                .update_index(segment.segment(), Some(*block_range.end()))?;
        }

        let elapsed = start.elapsed(); // TODO(alexey): track in metrics
        debug!(target: "static_file", ?targets, ?elapsed, "StaticFileProducer finished");

        self.event_sender
            .notify(StaticFileProducerEvent::Finished { targets: targets.clone(), elapsed });

        Ok(targets)
    }

    /// Copies data from database to static files according to
    /// [stage checkpoints](reth_stages_types::StageCheckpoint).
    ///
    /// Returns highest block numbers for all static file segments.
    pub fn copy_to_static_files(&self) -> ProviderResult<HighestStaticFiles> {
        let provider = self.provider.database_provider_ro()?;
        let stages_checkpoints = [StageId::Headers, StageId::Execution, StageId::Bodies]
            .into_iter()
            .map(|stage| provider.get_stage_checkpoint(stage).map(|c| c.map(|c| c.block_number)))
            .collect::<Result<Vec<_>, _>>()?;

        let highest_static_files = HighestStaticFiles {
            headers: stages_checkpoints[0],
            receipts: stages_checkpoints[1],
            transactions: stages_checkpoints[2],
            block_meta: stages_checkpoints[2],
        };
        let targets = self.get_static_file_targets(highest_static_files)?;
        self.run(targets)?;

        Ok(highest_static_files)
    }

    /// Returns a static file targets at the provided finalized block numbers per segment.
    /// The target is determined by the check against highest `static_files` using
    /// [`reth_provider::providers::StaticFileProvider::get_highest_static_files`].
    pub fn get_static_file_targets(
        &self,
        finalized_block_numbers: HighestStaticFiles,
    ) -> ProviderResult<StaticFileTargets> {
        let highest_static_files = self.provider.static_file_provider().get_highest_static_files();

        let targets = StaticFileTargets {
            headers: finalized_block_numbers.headers.and_then(|finalized_block_number| {
                self.get_static_file_target(highest_static_files.headers, finalized_block_number)
            }),
            // StaticFile receipts only if they're not pruned according to the user configuration
            receipts: if self.prune_modes.receipts.is_none() &&
                self.prune_modes.receipts_log_filter.is_empty()
            {
                finalized_block_numbers.receipts.and_then(|finalized_block_number| {
                    self.get_static_file_target(
                        highest_static_files.receipts,
                        finalized_block_number,
                    )
                })
            } else {
                None
            },
            transactions: finalized_block_numbers.transactions.and_then(|finalized_block_number| {
                self.get_static_file_target(
                    highest_static_files.transactions,
                    finalized_block_number,
                )
            }),
            block_meta: finalized_block_numbers.block_meta.and_then(|finalized_block_number| {
                self.get_static_file_target(highest_static_files.block_meta, finalized_block_number)
            }),
        };

        trace!(
            target: "static_file",
            ?finalized_block_numbers,
            ?highest_static_files,
            ?targets,
            any = %targets.any(),
            "StaticFile targets"
        );

        Ok(targets)
    }

    fn get_static_file_target(
        &self,
        highest_static_file: Option<BlockNumber>,
        finalized_block_number: BlockNumber,
    ) -> Option<RangeInclusive<BlockNumber>> {
        let range = highest_static_file.map_or(0, |block| block + 1)..=finalized_block_number;
        (!range.is_empty()).then_some(range)
    }
}

#[cfg(test)]
mod tests {
    use crate::static_file_producer::{
        StaticFileProducer, StaticFileProducerInner, StaticFileTargets,
    };
    use alloy_primitives::{B256, U256};
    use assert_matches::assert_matches;
    use reth_db_api::{database::Database, transaction::DbTx};
    use reth_provider::{
        providers::StaticFileWriter, test_utils::MockNodeTypesWithDB, ProviderError,
        ProviderFactory, StaticFileProviderFactory,
    };
    use reth_prune_types::PruneModes;
    use reth_stages::test_utils::{StorageKind, TestStageDB};
    use reth_static_file_types::{HighestStaticFiles, StaticFileSegment};
    use reth_testing_utils::generators::{
        self, random_block_range, random_receipt, BlockRangeParams,
    };
    use std::{sync::mpsc::channel, time::Duration};
    use tempfile::TempDir;

    fn setup() -> (ProviderFactory<MockNodeTypesWithDB>, TempDir) {
        let mut rng = generators::rng();
        let db = TestStageDB::default();

        let blocks = random_block_range(
            &mut rng,
            0..=3,
            BlockRangeParams { parent: Some(B256::ZERO), tx_count: 2..3, ..Default::default() },
        );
        db.insert_blocks(blocks.iter(), StorageKind::Database(None)).expect("insert blocks");
        // Unwind headers from static_files and manually insert them into the database, so we're
        // able to check that static_file_producer works
        let static_file_provider = db.factory.static_file_provider();
        let mut static_file_writer = static_file_provider
            .latest_writer(StaticFileSegment::Headers)
            .expect("get static file writer for headers");
        static_file_writer.prune_headers(blocks.len() as u64).unwrap();
        static_file_writer.commit().expect("prune headers");

        let tx = db.factory.db_ref().tx_mut().expect("init tx");
        for block in &blocks {
            TestStageDB::insert_header(None, &tx, block.sealed_header(), U256::ZERO)
                .expect("insert block header");
        }
        tx.commit().expect("commit tx");

        let mut receipts = Vec::new();
        for block in &blocks {
            for transaction in &block.body().transactions {
                receipts.push((
                    receipts.len() as u64,
                    random_receipt(&mut rng, transaction, Some(0), None),
                ));
            }
        }
        db.insert_receipts(receipts).expect("insert receipts");

        let provider_factory = db.factory;
        (provider_factory, db.temp_static_files_dir)
    }

    #[test]
    fn run() {
        let (provider_factory, _temp_static_files_dir) = setup();

        let static_file_producer =
            StaticFileProducerInner::new(provider_factory.clone(), PruneModes::default());

        let targets = static_file_producer
            .get_static_file_targets(HighestStaticFiles {
                headers: Some(1),
                receipts: Some(1),
                transactions: Some(1),
                block_meta: None,
            })
            .expect("get static file targets");
        assert_eq!(
            targets,
            StaticFileTargets {
                headers: Some(0..=1),
                receipts: Some(0..=1),
                transactions: Some(0..=1),
                block_meta: None
            }
        );
        assert_matches!(static_file_producer.run(targets), Ok(_));
        assert_eq!(
            provider_factory.static_file_provider().get_highest_static_files(),
            HighestStaticFiles {
                headers: Some(1),
                receipts: Some(1),
                transactions: Some(1),
                block_meta: None
            }
        );

        let targets = static_file_producer
            .get_static_file_targets(HighestStaticFiles {
                headers: Some(3),
                receipts: Some(3),
                transactions: Some(3),
                block_meta: None,
            })
            .expect("get static file targets");
        assert_eq!(
            targets,
            StaticFileTargets {
                headers: Some(2..=3),
                receipts: Some(2..=3),
                transactions: Some(2..=3),
                block_meta: None
            }
        );
        assert_matches!(static_file_producer.run(targets), Ok(_));
        assert_eq!(
            provider_factory.static_file_provider().get_highest_static_files(),
            HighestStaticFiles {
                headers: Some(3),
                receipts: Some(3),
                transactions: Some(3),
                block_meta: None
            }
        );

        let targets = static_file_producer
            .get_static_file_targets(HighestStaticFiles {
                headers: Some(4),
                receipts: Some(4),
                transactions: Some(4),
                block_meta: None,
            })
            .expect("get static file targets");
        assert_eq!(
            targets,
            StaticFileTargets {
                headers: Some(4..=4),
                receipts: Some(4..=4),
                transactions: Some(4..=4),
                block_meta: None
            }
        );
        assert_matches!(
            static_file_producer.run(targets),
            Err(ProviderError::BlockBodyIndicesNotFound(4))
        );
        assert_eq!(
            provider_factory.static_file_provider().get_highest_static_files(),
            HighestStaticFiles {
                headers: Some(3),
                receipts: Some(3),
                transactions: Some(3),
                block_meta: None
            }
        );
    }

    /// Tests that a cloneable [`StaticFileProducer`] type is not susceptible to any race condition.
    #[test]
    fn only_one() {
        let (provider_factory, _temp_static_files_dir) = setup();

        let static_file_producer = StaticFileProducer::new(provider_factory, PruneModes::default());

        let (tx, rx) = channel();

        for i in 0..5 {
            let producer = static_file_producer.clone();
            let tx = tx.clone();

            std::thread::spawn(move || {
                let locked_producer = producer.lock();
                if i == 0 {
                    // Let other threads spawn as well.
                    std::thread::sleep(Duration::from_millis(100));
                }
                let targets = locked_producer
                    .get_static_file_targets(HighestStaticFiles {
                        headers: Some(1),
                        receipts: Some(1),
                        transactions: Some(1),
                        block_meta: None,
                    })
                    .expect("get static file targets");
                assert_matches!(locked_producer.run(targets.clone()), Ok(_));
                tx.send(targets).unwrap();
            });
        }

        drop(tx);

        let mut only_one = Some(());
        for target in rx {
            // Only the first spawn should have any meaningful target.
            assert!(only_one.take().is_some_and(|_| target.any()) || !target.any())
        }
    }
}
