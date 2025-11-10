use crate::{
    segments::{PruneInput, Segment},
    PrunerError,
};
use alloy_primitives::BlockNumber;
use reth_provider::{BlockReader, PruneCheckpointReader, StaticFileProviderFactory};
use reth_prune_types::{
    PruneInterruptReason, PruneMode, PruneProgress, PrunePurpose, PruneSegment, SegmentOutput,
    SegmentOutputCheckpoint,
};
use reth_static_file_types::StaticFileSegment;
use tracing::debug;

/// Segment responsible for pruning transactions in static files.
///
/// This segment is controlled by the `bodies_history` configuration.
#[derive(Debug)]
pub struct Bodies {
    mode: PruneMode,
    /// Transaction lookup prune mode. Used to determine if we need to wait for tx lookup pruning
    /// before deleting transaction bodies.
    tx_lookup_mode: Option<PruneMode>,
}

impl Bodies {
    /// Creates a new [`Bodies`] segment with the given prune mode and optional transaction lookup
    /// prune mode for coordination.
    pub const fn new(mode: PruneMode, tx_lookup_mode: Option<PruneMode>) -> Self {
        Self { mode, tx_lookup_mode }
    }

    /// Returns the next best block that bodies can prune up to considering the transaction lookup
    /// pruning configuration (if any) and progress.
    ///
    /// Returns `None` if there's no block available to prune (e.g., waiting on `tx_lookup`).
    fn next_bodies_prune_target<Provider>(
        &self,
        provider: &Provider,
        input: &PruneInput,
    ) -> Result<Option<BlockNumber>, PrunerError>
    where
        Provider: PruneCheckpointReader,
    {
        let Some(tx_lookup_mode) = self.tx_lookup_mode else { return Ok(Some(input.to_block)) };

        let tx_lookup_checkpoint = provider
            .get_prune_checkpoint(PruneSegment::TransactionLookup)?
            .and_then(|cp| cp.block_number);

        // Determine the safe prune target, if any.
        let to_block = match tx_lookup_mode.next_pruned_block(tx_lookup_checkpoint) {
            None => Some(input.to_block), /* tx_lookup is done */
            Some(tx_lookup_next) if tx_lookup_next > input.to_block => Some(input.to_block), /* tx_lookup is ahead */
            Some(tx_lookup_next) => {
                // We can only prune up to the block before tx_lookup's next block.
                // If next is 0, there's nothing safe to prune yet.
                let Some(safe) = tx_lookup_next.checked_sub(1) else {
                    return Ok(None);
                };

                if input.previous_checkpoint.is_some_and(|cp| cp.block_number.unwrap_or(0) >= safe)
                {
                    // we have pruned what we can
                    return Ok(None)
                }

                debug!(
                    target: "pruner",
                    to_block = input.to_block,
                    safe,
                    "Bodies pruning limited by tx_lookup progress"
                );
                Some(safe)
            }
        };

        Ok(to_block)
    }
}

impl<Provider> Segment<Provider> for Bodies
where
    Provider: StaticFileProviderFactory + BlockReader + PruneCheckpointReader,
{
    fn segment(&self) -> PruneSegment {
        PruneSegment::Bodies
    }

    fn mode(&self) -> Option<PruneMode> {
        Some(self.mode)
    }

    fn purpose(&self) -> PrunePurpose {
        PrunePurpose::User
    }

    fn prune(&self, provider: &Provider, input: PruneInput) -> Result<SegmentOutput, PrunerError> {
        let Some(to_block) = self.next_bodies_prune_target(provider, &input)? else {
            debug!(
                to_block = input.to_block,
                "Transaction lookup still has work to be done up to target block"
            );
            return Ok(SegmentOutput::not_done(
                PruneInterruptReason::WaitingOnSegment(PruneSegment::TransactionLookup),
                input.previous_checkpoint.map(SegmentOutputCheckpoint::from_prune_checkpoint),
            ));
        };

        let deleted_headers = provider
            .static_file_provider()
            .delete_segment_below_block(StaticFileSegment::Transactions, to_block + 1)?;

        if deleted_headers.is_empty() {
            return Ok(SegmentOutput {
                progress: PruneProgress::Finished,
                pruned: 0,
                checkpoint: input
                    .previous_checkpoint
                    .map(SegmentOutputCheckpoint::from_prune_checkpoint),
            })
        }

        let tx_ranges = deleted_headers.iter().filter_map(|header| header.tx_range());

        let pruned = tx_ranges.clone().map(|range| range.len()).sum::<u64>() as usize;

        // The highest block number in the deleted files is the actual checkpoint. to_block might
        // refer to a block in the middle of a undeleted file.
        let checkpoint_block = deleted_headers
            .iter()
            .filter_map(|header| header.block_range())
            .map(|range| range.end())
            .max();

        Ok(SegmentOutput {
            progress: PruneProgress::Finished,
            pruned,
            checkpoint: Some(SegmentOutputCheckpoint {
                block_number: checkpoint_block,
                tx_number: tx_ranges.map(|range| range.end()).max(),
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Pruner;
    use alloy_primitives::BlockNumber;
    use reth_exex_types::FinishedExExHeight;
    use reth_provider::{
        test_utils::{create_test_provider_factory, MockNodeTypesWithDB},
        DBProvider, DatabaseProviderFactory, ProviderFactory, PruneCheckpointWriter,
        StaticFileWriter,
    };
    use reth_prune_types::{PruneMode, PruneProgress, PruneSegment};
    use reth_static_file_types::{
        SegmentHeader, SegmentRangeInclusive, StaticFileSegment, DEFAULT_BLOCKS_PER_STATIC_FILE,
    };

    /// Creates empty static file jars at 500k block intervals up to the tip block.
    ///
    /// Each jar contains sequential transaction ranges for testing deletion logic.
    fn setup_static_file_jars<P: StaticFileProviderFactory>(provider: &P, tip_block: u64) {
        let num_jars = (tip_block + 1) / DEFAULT_BLOCKS_PER_STATIC_FILE;
        let txs_per_jar = 1000;
        let static_file_provider = provider.static_file_provider();

        let mut writer =
            static_file_provider.latest_writer(StaticFileSegment::Transactions).unwrap();

        for jar_idx in 0..num_jars {
            let block_start = jar_idx * DEFAULT_BLOCKS_PER_STATIC_FILE;
            let block_end = ((jar_idx + 1) * DEFAULT_BLOCKS_PER_STATIC_FILE - 1).min(tip_block);

            let tx_start = jar_idx * txs_per_jar;
            let tx_end = tx_start + txs_per_jar - 1;

            *writer.user_header_mut() = SegmentHeader::new(
                SegmentRangeInclusive::new(block_start, block_end),
                Some(SegmentRangeInclusive::new(block_start, block_end)),
                Some(SegmentRangeInclusive::new(tx_start, tx_end)),
                StaticFileSegment::Transactions,
            );

            writer.inner().set_dirty();
            writer.commit().expect("commit empty jar");

            if jar_idx < num_jars - 1 {
                writer.increment_block(block_end + 1).expect("increment block");
            }
        }

        static_file_provider.initialize_index().expect("initialize index");
    }

    struct TestCase {
        tx_lookup_mode: Option<PruneMode>,
        tx_lookup_checkpoint_block: Option<BlockNumber>,
        bodies_mode: PruneMode,
        expected_pruned: usize,
        expected_lowest_block: Option<BlockNumber>,
        expected_progress: PruneProgress,
    }

    impl TestCase {
        fn new() -> Self {
            Self {
                tx_lookup_mode: None,
                tx_lookup_checkpoint_block: None,
                bodies_mode: PruneMode::Full,
                expected_pruned: 0,
                expected_lowest_block: None,
                expected_progress: PruneProgress::Finished,
            }
        }

        fn with_bodies_mode(mut self, mode: PruneMode) -> Self {
            self.bodies_mode = mode;
            self
        }

        fn with_expected_pruned(mut self, pruned: usize) -> Self {
            self.expected_pruned = pruned;
            self
        }

        fn with_expected_progress(mut self, progress: PruneProgress) -> Self {
            self.expected_progress = progress;
            self
        }

        fn with_lowest_block(mut self, block: BlockNumber) -> Self {
            self.expected_lowest_block = Some(block);
            self
        }

        fn with_tx_lookup(mut self, mode: PruneMode, checkpoint: Option<BlockNumber>) -> Self {
            self.tx_lookup_mode = Some(mode);
            self.tx_lookup_checkpoint_block = checkpoint;
            self
        }
    }

    fn run_prune_test(
        factory: &ProviderFactory<MockNodeTypesWithDB>,
        test_case: TestCase,
        tip: BlockNumber,
    ) {
        let (_, finished_exex_height_rx) = tokio::sync::watch::channel(FinishedExExHeight::NoExExs);

        // Capture highest block before pruning
        let static_provider = factory.static_file_provider();
        let highest_before =
            static_provider.get_highest_static_file_block(StaticFileSegment::Transactions);

        // Set up tx_lookup checkpoint if provided
        if let Some(checkpoint_block) = test_case.tx_lookup_checkpoint_block {
            let provider = factory.database_provider_rw().unwrap();
            provider
                .save_prune_checkpoint(
                    PruneSegment::TransactionLookup,
                    reth_prune_types::PruneCheckpoint {
                        block_number: Some(checkpoint_block),
                        tx_number: None,
                        prune_mode: test_case.tx_lookup_mode.unwrap(),
                    },
                )
                .unwrap();
            provider.commit().unwrap();
        }

        let bodies = Bodies::new(test_case.bodies_mode, test_case.tx_lookup_mode);
        let segments: Vec<Box<dyn Segment<_>>> = vec![Box::new(bodies)];

        let mut pruner = Pruner::new_with_factory(
            factory.clone(),
            segments,
            5,
            10000,
            None,
            finished_exex_height_rx,
        );

        let result = pruner.run(tip).expect("pruner run");

        assert_eq!(result.progress, test_case.expected_progress);
        assert_eq!(result.segments.len(), 1);

        let (segment, output) = &result.segments[0];
        assert_eq!(*segment, PruneSegment::Bodies);
        assert_eq!(output.pruned, test_case.expected_pruned);

        if let Some(expected_lowest) = test_case.expected_lowest_block {
            let static_provider = factory.static_file_provider();
            assert_eq!(
                static_provider.get_lowest_static_file_block(StaticFileSegment::Transactions),
                Some(expected_lowest)
            );
            assert_eq!(
                static_provider.get_highest_static_file_block(StaticFileSegment::Transactions),
                highest_before
            );
        }
    }

    #[test]
    fn bodies_prune_through_pruner() {
        let factory = create_test_provider_factory();
        let tip = 2_499_999;
        setup_static_file_jars(&factory, tip);

        let (_, _finished_exex_height_rx) =
            tokio::sync::watch::channel(FinishedExExHeight::NoExExs);

        let test_cases = vec![
            // Test 1: PruneMode::Before(750_000) → deletes jar 0 (0-499_999)
            // Checkpoint 499_999 != target 749_999 -> HasMoreData
            TestCase::new()
                .with_bodies_mode(PruneMode::Before(750_000))
                .with_expected_pruned(1000)
                .with_lowest_block(999_999),
            // Test 2: PruneMode::Before(850_000) → no deletion (jar 1: 500_000-999_999 contains
            // target)
            TestCase::new().with_bodies_mode(PruneMode::Before(850_000)).with_lowest_block(999_999),
            // Test 3: PruneMode::Before(1_599_999) → deletes jars 0 and 1 (0-999_999)
            // Checkpoint 999_999 != target 1_599_998 -> HasMoreData
            TestCase::new()
                .with_bodies_mode(PruneMode::Before(1_599_999))
                .with_expected_pruned(2000)
                .with_lowest_block(1_999_999),
            // Test 4: PruneMode::Distance(500_000) with tip=2_499_999 → deletes jar 3
            // (1_500_000-1_999_999) Checkpoint 1_999_999 == target 1_999_999 ->
            // Finished
            TestCase::new()
                .with_bodies_mode(PruneMode::Distance(500_000))
                .with_expected_pruned(1000)
                .with_lowest_block(2_499_999),
            // Test 5: PruneMode::Before(2_300_000) → no deletion (jar 4: 2_000_000-2_499_999
            // contains target)
            TestCase::new()
                .with_bodies_mode(PruneMode::Before(2_300_000))
                .with_lowest_block(2_499_999),
        ];

        for test_case in test_cases {
            run_prune_test(&factory, test_case, tip);
        }
    }

    #[test]
    fn checkpoint_reflects_deleted_files_not_target() {
        // Test that checkpoint is set to the highest deleted block, not to_block.
        // When to_block falls in the middle of an undeleted file, checkpoint should reflect
        // what was actually deleted.
        let factory = create_test_provider_factory();
        let tip = 1_499_999;
        setup_static_file_jars(&factory, tip);

        // Use PruneMode::Before(900_000) which targets 899_999.
        // This should delete jar 0 (0-499_999) since it's entirely below the target.
        // Jar 1 (500_000-999_999) contains the target, so it won't be deleted.
        // Checkpoint should be 499_999 (end of jar 0), not 899_999 (to_block).
        let bodies = Bodies::new(PruneMode::Before(900_000), None);
        let segments: Vec<Box<dyn Segment<_>>> = vec![Box::new(bodies)];

        let (_, finished_exex_height_rx) = tokio::sync::watch::channel(FinishedExExHeight::NoExExs);

        let mut pruner =
            Pruner::new_with_factory(factory, segments, 5, 10000, None, finished_exex_height_rx);

        let result = pruner.run(tip).expect("pruner run");

        assert_eq!(result.progress, PruneProgress::Finished);
        assert_eq!(result.segments.len(), 1);

        let (segment, output) = &result.segments[0];
        assert_eq!(*segment, PruneSegment::Bodies);

        // Verify checkpoint is set to the end of deleted jar (499_999), not to_block (899_999)
        let checkpoint_block = output.checkpoint.as_ref().and_then(|cp| cp.block_number);
        assert_eq!(
            checkpoint_block,
            Some(499_999),
            "Checkpoint should be 499_999 (end of deleted jar 0), not 899_999 (to_block)"
        );
    }

    #[test]
    fn min_block_updated_on_sync() {
        // Regression test: update_index must update min_block to prevent stale values
        // that can cause pruner to incorrectly delete static files when PruneMode::Before(0) is
        // used.

        struct MinBlockTestCase {
            // Block range
            initial_range: Option<SegmentRangeInclusive>,
            updated_range: SegmentRangeInclusive,
            // Min block
            expected_before_update: Option<BlockNumber>,
            expected_after_update: BlockNumber,
            // Test delete_segment_below_block with this value
            delete_below_block: BlockNumber,
            // Expected number of deleted segments
            expected_deleted: usize,
        }

        let test_cases = vec![
            // Test 1: Empty initial state (None) -> syncs to block 100
            MinBlockTestCase {
                initial_range: None,
                updated_range: SegmentRangeInclusive::new(0, 100),
                expected_before_update: None,
                expected_after_update: 100,
                delete_below_block: 1,
                expected_deleted: 0,
            },
            // Test 2: Genesis state [0..=0] -> syncs to block 100 (eg. op-reth node after op-reth
            // init-state)
            MinBlockTestCase {
                initial_range: Some(SegmentRangeInclusive::new(0, 0)),
                updated_range: SegmentRangeInclusive::new(0, 100),
                expected_before_update: Some(0),
                expected_after_update: 100,
                delete_below_block: 1,
                expected_deleted: 0,
            },
            // Test 3: Existing state [0..=50] -> syncs to block 200
            MinBlockTestCase {
                initial_range: Some(SegmentRangeInclusive::new(0, 50)),
                updated_range: SegmentRangeInclusive::new(0, 200),
                expected_before_update: Some(50),
                expected_after_update: 200,
                delete_below_block: 150,
                expected_deleted: 0,
            },
        ];

        for (
            idx,
            MinBlockTestCase {
                initial_range,
                updated_range,
                expected_before_update,
                expected_after_update,
                delete_below_block,
                expected_deleted,
            },
        ) in test_cases.into_iter().enumerate()
        {
            let factory = create_test_provider_factory();
            let static_provider = factory.static_file_provider();

            let mut writer =
                static_provider.latest_writer(StaticFileSegment::Transactions).unwrap();

            // Set up initial state if provided
            if let Some(initial_range) = initial_range {
                *writer.user_header_mut() = SegmentHeader::new(
                    initial_range,
                    Some(initial_range),
                    Some(initial_range),
                    StaticFileSegment::Transactions,
                );
                writer.inner().set_dirty();
                writer.commit().unwrap();
                static_provider.initialize_index().unwrap();
            }

            // Verify initial state
            assert_eq!(
                static_provider.get_lowest_static_file_block(StaticFileSegment::Transactions),
                expected_before_update,
                "Test case {}: Initial min_block mismatch",
                idx
            );

            // Update to new range
            *writer.user_header_mut() = SegmentHeader::new(
                updated_range,
                Some(updated_range),
                Some(updated_range),
                StaticFileSegment::Transactions,
            );
            writer.inner().set_dirty();
            writer.commit().unwrap(); // update_index is called inside

            // Verify min_block was updated (not stuck at stale value)
            assert_eq!(
                static_provider.get_lowest_static_file_block(StaticFileSegment::Transactions),
                Some(expected_after_update),
                "Test case {}: min_block should be updated to {} (not stuck at stale value)",
                idx,
                expected_after_update
            );

            // Verify delete_segment_below_block behaves correctly with updated min_block
            let deleted = static_provider
                .delete_segment_below_block(StaticFileSegment::Transactions, delete_below_block)
                .unwrap();

            assert_eq!(deleted.len(), expected_deleted);
        }
    }

    #[test]
    fn bodies_with_tx_lookup_coordination() {
        // Test that bodies pruning correctly coordinates with tx lookup pruning
        // Using tip = 1_523_000 creates 4 static file jars:
        // - Jar 0: blocks 0-499_999, txs 0-999
        // - Jar 1: blocks 500_000-999_999, txs 1000-1999
        // - Jar 2: blocks 1_000_000-1_499_999, txs 2000-2999
        // - Jar 3: blocks 1_500_000-1_523_000, txs 3000-3999
        let tip = 1_523_000;

        let test_cases = vec![
            // Scenario 1: tx_lookup disabled, bodies can prune freely (deletes jar 0)
            // Checkpoint is 499_999 (end of jar 0), target is 599_999, so HasMoreData
            TestCase::new()
                .with_bodies_mode(PruneMode::Before(600_000))
                .with_expected_pruned(1000)
                .with_lowest_block(999_999),
            // Scenario 2: tx_lookup enabled but not run yet, bodies cannot prune
            TestCase::new()
                .with_tx_lookup(PruneMode::Before(600_000), None)
                .with_bodies_mode(PruneMode::Before(600_000))
                .with_expected_progress(PruneProgress::HasMoreData(
                    PruneInterruptReason::WaitingOnSegment(PruneSegment::TransactionLookup),
                ))
                .with_lowest_block(499_999), // No jars deleted, jar 0 ends at 499_999
            // Scenario 3: tx_lookup caught up to its target, bodies can prune freely
            // Deletes jar 0, checkpoint is 499_999, target is 599_999 -> HasMoreData
            TestCase::new()
                .with_tx_lookup(PruneMode::Before(600_000), Some(599_999))
                .with_bodies_mode(PruneMode::Before(600_000))
                .with_expected_pruned(1000)
                .with_lowest_block(999_999),
            // Scenario 4: tx_lookup behind its target, bodies limited to tx_lookup checkpoint
            // tx_lookup should prune up to 599_999, but checkpoint is only at 250_000
            // bodies wants to prune up to 599_999, but limited to 250_000
            // No jars deleted because jar 0 (0-499_999) ends beyond 250_000
            TestCase::new()
                .with_tx_lookup(PruneMode::Before(600_000), Some(250_000))
                .with_bodies_mode(PruneMode::Before(600_000))
                .with_lowest_block(499_999), // No jars deleted
            // Scenario 5: Both use Distance, tx_lookup caught up
            // With tip=1_523_000, Distance(500_000) targets block 1_023_000
            // Deletes jars 0 and 1, checkpoint is 999_999, target is 1_023_000 -> HasMoreData
            TestCase::new()
                .with_tx_lookup(PruneMode::Distance(500_000), Some(1_023_000))
                .with_bodies_mode(PruneMode::Distance(500_000))
                .with_expected_pruned(2000)
                .with_lowest_block(1_499_999),
            // Scenario 6: Both use Distance, tx_lookup less aggressive (bigger distance) than
            // bodies With tip=1_523_000:
            // - tx_lookup: Distance(1_000_000) targets block 523_000, checkpoint at 523_000
            // - bodies: Distance(500_000) targets block 1_023_000
            // Bodies can prune up to what tx_lookup has finished (523_000), deleting jar 0
            // Checkpoint is 499_999, target is 1_023_000 -> HasMoreData
            TestCase::new()
                .with_tx_lookup(PruneMode::Distance(1_000_000), Some(523_000))
                .with_bodies_mode(PruneMode::Distance(500_000))
                .with_expected_pruned(1000) // Jar 0 deleted
                .with_lowest_block(999_999), // Jar 0 (0-499_999) deleted
            // Scenario 7: tx_lookup more aggressive than bodies (deletes jar 0 and 1)
            // tx_lookup: Before(1_100_000) -> prune up to 1_099_999
            // bodies: Before(1_100_000) -> wants to prune up to 1_099_999
            // Checkpoint is 999_999, target is 1_099_999 -> HasMoreData
            TestCase::new()
                .with_tx_lookup(PruneMode::Before(1_100_000), Some(1_099_999))
                .with_bodies_mode(PruneMode::Before(1_100_000))
                .with_expected_pruned(2000)
                .with_lowest_block(1_499_999), // Jars 0 and 1 deleted
            // Scenario 8: tx_lookup has lower target than bodies, but is done
            // tx_lookup: Before(600_000) -> prune up to 599_999 (checkpoint at 599_999, DONE)
            // bodies: Before(1_100_000) -> wants to prune up to 1_099_999
            // Since tx_lookup is done (next_pruned_block returns None), bodies can prune freely
            // Checkpoint is 999_999, target is 1_099_999 -> HasMoreData
            TestCase::new()
                .with_tx_lookup(PruneMode::Before(600_000), Some(599_999))
                .with_bodies_mode(PruneMode::Before(1_100_000))
                .with_expected_pruned(2000)
                .with_lowest_block(1_499_999), // Jars 0 and 1 deleted
            // Scenario 9: Perfect alignment - checkpoint equals target
            // bodies: Before(1_000_000) -> targets 999_999
            // Deletes jars 0 and 1 (0-999_999), checkpoint is 999_999 which equals target ->
            // Finished
            TestCase::new()
                .with_bodies_mode(PruneMode::Before(1_000_000))
                .with_expected_pruned(2000)
                .with_expected_progress(PruneProgress::Finished)
                .with_lowest_block(1_499_999), // Jars 0 and 1 deleted
        ];

        for test_case in test_cases {
            let factory = create_test_provider_factory();
            setup_static_file_jars(&factory, tip);
            run_prune_test(&factory, test_case, tip);
        }
    }
}
