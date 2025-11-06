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
use std::ops::Not;
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

    /// Returns whether transaction lookup pruning is done with blocks up to the target.
    fn is_tx_lookup_done<Provider>(
        &self,
        provider: &Provider,
        target: BlockNumber,
    ) -> Result<bool, PrunerError>
    where
        Provider: PruneCheckpointReader,
    {
        let Some(tx_lookup_mode) = self.tx_lookup_mode else { return Ok(true) };

        let tx_lookup_checkpoint = provider
            .get_prune_checkpoint(PruneSegment::TransactionLookup)?
            .and_then(|cp| cp.block_number);

        // Check if tx_lookup will eventually need to prune any blocks <= our target
        Ok(tx_lookup_mode
            .next_pruned_block(tx_lookup_checkpoint)
            .is_some_and(|next_block| next_block <= target)
            .not())
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
        if !self.is_tx_lookup_done(provider, input.to_block)? {
            debug!(
                to_block = input.to_block,
                "Transaction lookup still has work to be done up to target block"
            );
            return Ok(SegmentOutput::not_done(
                PruneInterruptReason::WaitingOnSegment(PruneSegment::TransactionLookup),
                input.previous_checkpoint.map(SegmentOutputCheckpoint::from_prune_checkpoint),
            ))
        }

        let deleted_headers = provider
            .static_file_provider()
            .delete_segment_below_block(StaticFileSegment::Transactions, input.to_block + 1)?;

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

        Ok(SegmentOutput {
            progress: PruneProgress::Finished,
            pruned,
            checkpoint: Some(SegmentOutputCheckpoint {
                block_number: Some(input.to_block),
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

    struct PruneTestCase {
        prune_mode: PruneMode,
        expected_pruned: usize,
        expected_lowest_block: Option<BlockNumber>,
    }

    fn run_prune_test(
        factory: &ProviderFactory<MockNodeTypesWithDB>,
        finished_exex_height_rx: &tokio::sync::watch::Receiver<FinishedExExHeight>,
        test_case: PruneTestCase,
        tip: BlockNumber,
    ) {
        let bodies = Bodies::new(test_case.prune_mode, None);
        let segments: Vec<Box<dyn Segment<_>>> = vec![Box::new(bodies)];

        let mut pruner = Pruner::new_with_factory(
            factory.clone(),
            segments,
            5,
            10000,
            None,
            finished_exex_height_rx.clone(),
        );

        let result = pruner.run(tip).expect("pruner run");

        assert_eq!(result.progress, PruneProgress::Finished);
        assert_eq!(result.segments.len(), 1);

        let (segment, output) = &result.segments[0];
        assert_eq!(*segment, PruneSegment::Bodies);
        assert_eq!(output.pruned, test_case.expected_pruned);

        let static_provider = factory.static_file_provider();
        assert_eq!(
            static_provider.get_lowest_static_file_block(StaticFileSegment::Transactions),
            test_case.expected_lowest_block
        );
        assert_eq!(
            static_provider.get_highest_static_file_block(StaticFileSegment::Transactions),
            Some(tip)
        );
    }

    #[test]
    fn bodies_prune_through_pruner() {
        let factory = create_test_provider_factory();
        let tip = 2_499_999;
        setup_static_file_jars(&factory, tip);

        let (_, finished_exex_height_rx) = tokio::sync::watch::channel(FinishedExExHeight::NoExExs);

        let test_cases = vec![
            // Test 1: PruneMode::Before(750_000) → deletes jar 1 (0-499_999)
            PruneTestCase {
                prune_mode: PruneMode::Before(750_000),
                expected_pruned: 1000,
                expected_lowest_block: Some(999_999),
            },
            // Test 2: PruneMode::Before(850_000) → no deletion (jar 2: 500_000-999_999 contains
            // target)
            PruneTestCase {
                prune_mode: PruneMode::Before(850_000),
                expected_pruned: 0,
                expected_lowest_block: Some(999_999),
            },
            // Test 3: PruneMode::Before(1_599_999) → deletes jar 2 (500_000-999_999) and jar 3
            // (1_000_000-1_499_999)
            PruneTestCase {
                prune_mode: PruneMode::Before(1_599_999),
                expected_pruned: 2000,
                expected_lowest_block: Some(1_999_999),
            },
            // Test 4: PruneMode::Distance(500_000) with tip=2_499_999 → deletes jar 4
            // (1_500_000-1_999_999)
            PruneTestCase {
                prune_mode: PruneMode::Distance(500_000),
                expected_pruned: 1000,
                expected_lowest_block: Some(2_499_999),
            },
            // Test 5: PruneMode::Before(2_300_000) → no deletion (jar 5: 2_000_000-2_499_999
            // contains target)
            PruneTestCase {
                prune_mode: PruneMode::Before(2_300_000),
                expected_pruned: 0,
                expected_lowest_block: Some(2_499_999),
            },
        ];

        for test_case in test_cases {
            run_prune_test(&factory, &finished_exex_height_rx, test_case, tip);
        }
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

        struct TestCase {
            tx_lookup_mode: Option<PruneMode>,
            tx_lookup_checkpoint_block: Option<BlockNumber>,
            bodies_mode: PruneMode,
            expected_pruned: usize,
            expected_lowest_range_end: Option<BlockNumber>,
            expected_progress: PruneProgress,
        }

        let test_cases = vec![
            // Scenario 1: tx_lookup disabled, bodies can prune freely (deletes jar 0)
            TestCase {
                tx_lookup_mode: None,
                tx_lookup_checkpoint_block: None,
                bodies_mode: PruneMode::Before(600_000),
                expected_pruned: 1000,
                expected_lowest_range_end: Some(999_999),
                expected_progress: PruneProgress::Finished,
            },
            // Scenario 2: tx_lookup enabled but not run yet, bodies cannot prune
            TestCase {
                tx_lookup_mode: Some(PruneMode::Before(600_000)),
                tx_lookup_checkpoint_block: None,
                bodies_mode: PruneMode::Before(600_000),
                expected_pruned: 0,
                expected_lowest_range_end: Some(499_999), // No jars deleted, jar 0 ends at 499_999
                expected_progress: PruneProgress::HasMoreData(
                    PruneInterruptReason::WaitingOnSegment(PruneSegment::TransactionLookup),
                ),
            },
            // Scenario 3: tx_lookup caught up to its target, bodies can prune freely
            TestCase {
                tx_lookup_mode: Some(PruneMode::Before(600_000)),
                tx_lookup_checkpoint_block: Some(599_999),
                bodies_mode: PruneMode::Before(600_000),
                expected_pruned: 1000,
                expected_lowest_range_end: Some(999_999),
                expected_progress: PruneProgress::Finished,
            },
            // Scenario 4: tx_lookup behind its target, bodies limited to tx_lookup checkpoint
            // tx_lookup should prune up to 599_999, but checkpoint is only at 250_000
            // bodies wants to prune up to 599_999, but limited to 250_000
            // No jars deleted because jar 0 (0-499_999) ends beyond 250_000
            TestCase {
                tx_lookup_mode: Some(PruneMode::Before(600_000)),
                tx_lookup_checkpoint_block: Some(250_000),
                bodies_mode: PruneMode::Before(600_000),
                expected_pruned: 0,
                expected_lowest_range_end: Some(499_999), // No jars deleted
                expected_progress: PruneProgress::HasMoreData(
                    PruneInterruptReason::WaitingOnSegment(PruneSegment::TransactionLookup),
                ),
            },
            // Scenario 5: Both use Distance, tx_lookup caught up
            // With tip=1_523_000, Distance(500_000) targets block 1_023_000
            TestCase {
                tx_lookup_mode: Some(PruneMode::Distance(500_000)),
                tx_lookup_checkpoint_block: Some(1_023_000),
                bodies_mode: PruneMode::Distance(500_000),
                expected_pruned: 2000,
                expected_lowest_range_end: Some(1_499_999),
                expected_progress: PruneProgress::Finished,
            },
            // Scenario 6: Both use Distance, tx_lookup less aggressive (bigger distance) than
            // bodies With tip=1_523_000:
            // - tx_lookup: Distance(1_000_000) targets block 523_000, checkpoint at 523_000
            // - bodies: Distance(500_000) targets block 1_023_000
            // So bodies must wait for tx_lookup to process more blocks
            TestCase {
                tx_lookup_mode: Some(PruneMode::Distance(1_000_000)),
                tx_lookup_checkpoint_block: Some(523_000),
                bodies_mode: PruneMode::Distance(500_000),
                expected_pruned: 0, // Can't prune, waiting on tx_lookup
                expected_lowest_range_end: Some(499_999), // No jars deleted
                expected_progress: PruneProgress::HasMoreData(
                    PruneInterruptReason::WaitingOnSegment(PruneSegment::TransactionLookup),
                ),
            },
            // Scenario 7: tx_lookup more aggressive than bodies (deletes jar 0, 1, and 2)
            // tx_lookup: Before(1_100_000) -> prune up to 1_099_999
            // bodies: Before(1_100_000) -> wants to prune up to 1_099_999
            TestCase {
                tx_lookup_mode: Some(PruneMode::Before(1_100_000)),
                tx_lookup_checkpoint_block: Some(1_099_999),
                bodies_mode: PruneMode::Before(1_100_000),
                expected_pruned: 2000,
                expected_lowest_range_end: Some(1_499_999), // Jars 0 and 1 deleted
                expected_progress: PruneProgress::Finished,
            },
            // Scenario 8: tx_lookup has lower target than bodies, but is done
            // tx_lookup: Before(600_000) -> prune up to 599_999 (checkpoint at 599_999, DONE)
            // bodies: Before(1_100_000) -> wants to prune up to 1_099_999
            // Since tx_lookup is done (next_pruned_block returns None), bodies can prune freely
            TestCase {
                tx_lookup_mode: Some(PruneMode::Before(600_000)),
                tx_lookup_checkpoint_block: Some(599_999),
                bodies_mode: PruneMode::Before(1_100_000),
                expected_pruned: 2000,
                expected_lowest_range_end: Some(1_499_999), // Jars 0 and 1 deleted
                expected_progress: PruneProgress::Finished,
            },
        ];

        let (_, finished_exex_height_rx) = tokio::sync::watch::channel(FinishedExExHeight::NoExExs);

        for (i, t) in test_cases.into_iter().enumerate() {
            let test_num = i + 1;

            // Reset factory state
            let factory = create_test_provider_factory();
            setup_static_file_jars(&factory, tip);

            // Set up tx lookup checkpoint if provided
            if let Some(checkpoint_block) = t.tx_lookup_checkpoint_block {
                let provider = factory.database_provider_rw().unwrap();
                provider
                    .save_prune_checkpoint(
                        PruneSegment::TransactionLookup,
                        reth_prune_types::PruneCheckpoint {
                            block_number: Some(checkpoint_block),
                            tx_number: Some(checkpoint_block), // Simplified for test
                            prune_mode: t.tx_lookup_mode.unwrap(),
                        },
                    )
                    .unwrap();
                provider.commit().unwrap();
            }

            // Run bodies pruning
            let bodies = Bodies::new(t.bodies_mode, t.tx_lookup_mode);
            let segments: Vec<Box<dyn Segment<_>>> = vec![Box::new(bodies)];

            let mut pruner = Pruner::new_with_factory(
                factory.clone(),
                segments,
                5,
                10000,
                None,
                finished_exex_height_rx.clone(),
            );

            let result = pruner.run(tip).expect("pruner run");

            assert_eq!(result.progress, t.expected_progress, "Test case {}", test_num);
            assert_eq!(result.segments.len(), 1, "Test case {}", test_num);

            let (segment, output) = &result.segments[0];
            assert_eq!(*segment, PruneSegment::Bodies, "Test case {}", test_num);
            assert_eq!(
                output.pruned, t.expected_pruned,
                "Test case {} - expected {} pruned, got {}",
                test_num, t.expected_pruned, output.pruned
            );

            let static_provider = factory.static_file_provider();
            assert_eq!(
                static_provider.get_lowest_static_file_block(StaticFileSegment::Transactions),
                t.expected_lowest_range_end,
                "Test case {} - expected lowest range end {:?}, got {:?}",
                test_num,
                t.expected_lowest_range_end,
                static_provider.get_lowest_static_file_block(StaticFileSegment::Transactions)
            );
        }
    }
}
