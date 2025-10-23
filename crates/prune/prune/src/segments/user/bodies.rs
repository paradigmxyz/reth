use crate::{
    segments::{PruneInput, Segment},
    PrunerError,
};
use reth_provider::{BlockReader, StaticFileProviderFactory};
use reth_prune_static_files::Bodies;
use reth_prune_types::{PruneMode, PrunePurpose, PruneSegment, SegmentOutput};

/// Segment adapter for pruning transactions in static files.
///
/// This wraps the implementation from `reth_prune_static_files` and implements
/// the `Segment` trait required by the pruner.
#[derive(Debug)]
pub struct BodiesAdapter {
    inner: Bodies,
}

impl BodiesAdapter {
    /// Creates a new [`BodiesAdapter`] segment with the given prune mode.
    pub const fn new(mode: PruneMode) -> Self {
        Self { inner: Bodies::new(mode) }
    }
}

impl<Provider> Segment<Provider> for BodiesAdapter
where
    Provider: StaticFileProviderFactory + BlockReader,
{
    fn segment(&self) -> PruneSegment {
        self.inner.segment()
    }

    fn mode(&self) -> Option<PruneMode> {
        self.inner.mode()
    }

    fn purpose(&self) -> PrunePurpose {
        self.inner.purpose()
    }

    fn prune(&self, provider: &Provider, input: PruneInput) -> Result<SegmentOutput, PrunerError> {
        Ok(self.inner.prune(provider, input.to_block)?)
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
        ProviderFactory, StaticFileWriter,
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
        let bodies_adapter = BodiesAdapter::new(test_case.prune_mode);
        let segments: Vec<Box<dyn Segment<_>>> = vec![Box::new(bodies_adapter)];

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
}
