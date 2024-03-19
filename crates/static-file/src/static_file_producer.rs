//! Support for producing static files.

use crate::{segments, segments::Segment, StaticFileProducerEvent};
use parking_lot::Mutex;
use rayon::prelude::*;
use reth_db::database::Database;
use reth_interfaces::RethResult;
use reth_primitives::{static_file::HighestStaticFiles, BlockNumber, PruneModes};
use reth_provider::{
    providers::{StaticFileProvider, StaticFileWriter},
    ProviderFactory,
};
use reth_tokio_util::EventListeners;
use std::{
    ops::{Deref, RangeInclusive},
    sync::Arc,
    time::Instant,
};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing::{debug, trace};

/// Result of [StaticFileProducerInner::run] execution.
pub type StaticFileProducerResult = RethResult<StaticFileTargets>;

/// The [StaticFileProducer] instance itself with the result of [StaticFileProducerInner::run]
pub type StaticFileProducerWithResult<DB> = (StaticFileProducer<DB>, StaticFileProducerResult);

/// Static File producer. It's a wrapper around [StaticFileProducer] that allows to share it
/// between threads.
#[derive(Debug, Clone)]
pub struct StaticFileProducer<DB>(Arc<Mutex<StaticFileProducerInner<DB>>>);

impl<DB: Database> StaticFileProducer<DB> {
    /// Creates a new [StaticFileProducer].
    pub fn new(
        provider_factory: ProviderFactory<DB>,
        static_file_provider: StaticFileProvider,
        prune_modes: PruneModes,
    ) -> Self {
        Self(Arc::new(Mutex::new(StaticFileProducerInner::new(
            provider_factory,
            static_file_provider,
            prune_modes,
        ))))
    }
}

impl<DB> Deref for StaticFileProducer<DB> {
    type Target = Arc<Mutex<StaticFileProducerInner<DB>>>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// Static File producer routine. See [StaticFileProducerInner::run] for more detailed description.
#[derive(Debug)]
pub struct StaticFileProducerInner<DB> {
    /// Provider factory
    provider_factory: ProviderFactory<DB>,
    /// Static File provider
    static_file_provider: StaticFileProvider,
    /// Pruning configuration for every part of the data that can be pruned. Set by user, and
    /// needed in [StaticFileProducerInner] to prevent attempting to move prunable data to static
    /// files. See [StaticFileProducerInner::get_static_file_targets].
    prune_modes: PruneModes,
    listeners: EventListeners<StaticFileProducerEvent>,
}

/// Static File targets, per data part, measured in [`BlockNumber`].
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct StaticFileTargets {
    headers: Option<RangeInclusive<BlockNumber>>,
    receipts: Option<RangeInclusive<BlockNumber>>,
    transactions: Option<RangeInclusive<BlockNumber>>,
}

impl StaticFileTargets {
    /// Returns `true` if any of the targets are [Some].
    pub fn any(&self) -> bool {
        self.headers.is_some() || self.receipts.is_some() || self.transactions.is_some()
    }

    // Returns `true` if all targets are either [`None`] or has beginning of the range equal to the
    // highest static_file.
    fn is_contiguous_to_highest_static_files(&self, static_files: HighestStaticFiles) -> bool {
        [
            (self.headers.as_ref(), static_files.headers),
            (self.receipts.as_ref(), static_files.receipts),
            (self.transactions.as_ref(), static_files.transactions),
        ]
        .iter()
        .all(|(target_block_range, highest_static_fileted_block)| {
            target_block_range.map_or(true, |target_block_range| {
                *target_block_range.start() ==
                    highest_static_fileted_block.map_or(0, |highest_static_fileted_block| {
                        highest_static_fileted_block + 1
                    })
            })
        })
    }
}

impl<DB: Database> StaticFileProducerInner<DB> {
    fn new(
        provider_factory: ProviderFactory<DB>,
        static_file_provider: StaticFileProvider,
        prune_modes: PruneModes,
    ) -> Self {
        Self { provider_factory, static_file_provider, prune_modes, listeners: Default::default() }
    }

    /// Listen for events on the static_file_producer.
    pub fn events(&mut self) -> UnboundedReceiverStream<StaticFileProducerEvent> {
        self.listeners.new_listener()
    }

    /// Run the static_file_producer.
    ///
    /// For each [Some] target in [StaticFileTargets], initializes a corresponding [Segment] and
    /// runs it with the provided block range using [StaticFileProvider] and a read-only
    /// database transaction from [ProviderFactory]. All segments are run in parallel.
    ///
    /// NOTE: it doesn't delete the data from database, and the actual deleting (aka pruning) logic
    /// lives in the `prune` crate.
    pub fn run(&mut self, targets: StaticFileTargets) -> StaticFileProducerResult {
        debug_assert!(targets.is_contiguous_to_highest_static_files(
            self.static_file_provider.get_highest_static_files()
        ));

        self.listeners.notify(StaticFileProducerEvent::Started { targets: targets.clone() });

        debug!(target: "static_file", ?targets, "StaticFileProducer started");
        let start = Instant::now();

        let mut segments = Vec::<(Box<dyn Segment<DB>>, RangeInclusive<BlockNumber>)>::new();

        if let Some(block_range) = targets.transactions.clone() {
            segments.push((Box::new(segments::Transactions), block_range));
        }
        if let Some(block_range) = targets.headers.clone() {
            segments.push((Box::new(segments::Headers), block_range));
        }
        if let Some(block_range) = targets.receipts.clone() {
            segments.push((Box::new(segments::Receipts), block_range));
        }

        segments.par_iter().try_for_each(|(segment, block_range)| -> RethResult<()> {
            debug!(target: "static_file", segment = %segment.segment(), ?block_range, "StaticFileProducer segment");
            let start = Instant::now();

            // Create a new database transaction on every segment to prevent long-lived read-only
            // transactions
            let provider = self.provider_factory.provider()?.disable_long_read_transaction_safety();
            segment.copy_to_static_files(provider, self.static_file_provider.clone(), block_range.clone())?;

            let elapsed = start.elapsed(); // TODO(alexey): track in metrics
            debug!(target: "static_file", segment = %segment.segment(), ?block_range, ?elapsed, "Finished StaticFileProducer segment");

            Ok(())
        })?;

        self.static_file_provider.commit()?;
        for (segment, block_range) in segments {
            self.static_file_provider.update_index(segment.segment(), Some(*block_range.end()))?;
        }

        let elapsed = start.elapsed(); // TODO(alexey): track in metrics
        debug!(target: "static_file", ?targets, ?elapsed, "StaticFileProducer finished");

        self.listeners
            .notify(StaticFileProducerEvent::Finished { targets: targets.clone(), elapsed });

        Ok(targets)
    }

    /// Returns a static file targets at the provided finalized block numbers per segment.
    /// The target is determined by the check against highest static_files using
    /// [StaticFileProvider::get_highest_static_files].
    pub fn get_static_file_targets(
        &self,
        finalized_block_numbers: HighestStaticFiles,
    ) -> RethResult<StaticFileTargets> {
        let highest_static_files = self.static_file_provider.get_highest_static_files();

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
    use assert_matches::assert_matches;
    use reth_db::{database::Database, test_utils::TempDatabase, transaction::DbTx, DatabaseEnv};
    use reth_interfaces::{
        provider::ProviderError,
        test_utils::{
            generators,
            generators::{random_block_range, random_receipt},
        },
        RethError,
    };
    use reth_primitives::{
        static_file::HighestStaticFiles, PruneModes, StaticFileSegment, B256, U256,
    };
    use reth_provider::{
        providers::{StaticFileProvider, StaticFileWriter},
        ProviderFactory,
    };
    use reth_stages::test_utils::{StorageKind, TestStageDB};
    use std::{
        sync::{mpsc::channel, Arc},
        time::Duration,
    };
    use tempfile::TempDir;

    fn setup() -> (ProviderFactory<Arc<TempDatabase<DatabaseEnv>>>, StaticFileProvider, TempDir) {
        let mut rng = generators::rng();
        let db = TestStageDB::default();

        let blocks = random_block_range(&mut rng, 0..=3, B256::ZERO, 2..3);
        db.insert_blocks(blocks.iter(), StorageKind::Database(None)).expect("insert blocks");
        // Unwind headers from static_files and manually insert them into the database, so we're
        // able to check that static_file_producer works
        db.factory
            .static_file_provider()
            .latest_writer(StaticFileSegment::Headers)
            .expect("get static file writer for headers")
            .prune_headers(blocks.len() as u64)
            .expect("prune headers");
        let tx = db.factory.db_ref().tx_mut().expect("init tx");
        blocks.iter().for_each(|block| {
            TestStageDB::insert_header(None, &tx, &block.header, U256::ZERO)
                .expect("insert block header");
        });
        tx.commit().expect("commit tx");

        let mut receipts = Vec::new();
        for block in &blocks {
            for transaction in &block.body {
                receipts
                    .push((receipts.len() as u64, random_receipt(&mut rng, transaction, Some(0))));
            }
        }
        db.insert_receipts(receipts).expect("insert receipts");

        let provider_factory = db.factory;
        let static_file_provider = provider_factory.static_file_provider();
        (provider_factory, static_file_provider, db.temp_static_files_dir)
    }

    #[test]
    fn run() {
        let (provider_factory, static_file_provider, _temp_static_files_dir) = setup();

        let mut static_file_producer = StaticFileProducerInner::new(
            provider_factory,
            static_file_provider.clone(),
            PruneModes::default(),
        );

        let targets = static_file_producer
            .get_static_file_targets(HighestStaticFiles {
                headers: Some(1),
                receipts: Some(1),
                transactions: Some(1),
            })
            .expect("get static file targets");
        assert_eq!(
            targets,
            StaticFileTargets {
                headers: Some(0..=1),
                receipts: Some(0..=1),
                transactions: Some(0..=1)
            }
        );
        assert_matches!(static_file_producer.run(targets), Ok(_));
        assert_eq!(
            static_file_provider.get_highest_static_files(),
            HighestStaticFiles { headers: Some(1), receipts: Some(1), transactions: Some(1) }
        );

        let targets = static_file_producer
            .get_static_file_targets(HighestStaticFiles {
                headers: Some(3),
                receipts: Some(3),
                transactions: Some(3),
            })
            .expect("get static file targets");
        assert_eq!(
            targets,
            StaticFileTargets {
                headers: Some(2..=3),
                receipts: Some(2..=3),
                transactions: Some(2..=3)
            }
        );
        assert_matches!(static_file_producer.run(targets), Ok(_));
        assert_eq!(
            static_file_provider.get_highest_static_files(),
            HighestStaticFiles { headers: Some(3), receipts: Some(3), transactions: Some(3) }
        );

        let targets = static_file_producer
            .get_static_file_targets(HighestStaticFiles {
                headers: Some(4),
                receipts: Some(4),
                transactions: Some(4),
            })
            .expect("get static file targets");
        assert_eq!(
            targets,
            StaticFileTargets {
                headers: Some(4..=4),
                receipts: Some(4..=4),
                transactions: Some(4..=4)
            }
        );
        assert_matches!(
            static_file_producer.run(targets),
            Err(RethError::Provider(ProviderError::BlockBodyIndicesNotFound(4)))
        );
        assert_eq!(
            static_file_provider.get_highest_static_files(),
            HighestStaticFiles { headers: Some(3), receipts: Some(3), transactions: Some(3) }
        );
    }

    /// Tests that a cloneable [`StaticFileProducer`] type is not susceptible to any race condition.
    #[test]
    fn only_one() {
        let (provider_factory, static_file_provider, _temp_static_files_dir) = setup();

        let static_file_producer =
            StaticFileProducer::new(provider_factory, static_file_provider, PruneModes::default());

        let (tx, rx) = channel();

        for i in 0..5 {
            let producer = static_file_producer.clone();
            let tx = tx.clone();

            std::thread::spawn(move || {
                let mut locked_producer = producer.lock();
                if i == 0 {
                    // Let other threads spawn as well.
                    std::thread::sleep(Duration::from_millis(100));
                }
                let targets = locked_producer
                    .get_static_file_targets(HighestStaticFiles {
                        headers: Some(1),
                        receipts: Some(1),
                        transactions: Some(1),
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
