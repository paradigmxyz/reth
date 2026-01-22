use crate::{
    segments::{self, PruneInput, Segment},
    PrunerError,
};
use reth_provider::{BlockReader, StaticFileProviderFactory};
use reth_prune_types::{PruneMode, PrunePurpose, PruneSegment, SegmentOutput};
use reth_static_file_types::StaticFileSegment;

/// Segment responsible for pruning storage change sets in static files.
#[derive(Debug)]
pub struct StorageChangeSets {
    mode: PruneMode,
}

impl StorageChangeSets {
    /// Creates a new [`StorageChangeSets`] segment with the given prune mode.
    pub const fn new(mode: PruneMode) -> Self {
        Self { mode }
    }
}

impl<Provider> Segment<Provider> for StorageChangeSets
where
    Provider: StaticFileProviderFactory + BlockReader,
{
    fn segment(&self) -> PruneSegment {
        PruneSegment::StorageChangeSets
    }

    fn mode(&self) -> Option<PruneMode> {
        Some(self.mode)
    }

    fn purpose(&self) -> PrunePurpose {
        PrunePurpose::User
    }

    fn prune(&self, provider: &Provider, input: PruneInput) -> Result<SegmentOutput, PrunerError> {
        segments::prune_static_files(provider, input, StaticFileSegment::StorageChangeSets)
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

    fn setup_static_file_jars<P: StaticFileProviderFactory>(provider: &P, tip_block: u64) {
        let num_jars = (tip_block + 1) / DEFAULT_BLOCKS_PER_STATIC_FILE;
        let static_file_provider = provider.static_file_provider();

        let mut writer =
            static_file_provider.latest_writer(StaticFileSegment::StorageChangeSets).unwrap();

        for jar_idx in 0..num_jars {
            let block_start = jar_idx * DEFAULT_BLOCKS_PER_STATIC_FILE;
            let block_end = ((jar_idx + 1) * DEFAULT_BLOCKS_PER_STATIC_FILE - 1).min(tip_block);

            *writer.user_header_mut() = SegmentHeader::new(
                SegmentRangeInclusive::new(block_start, block_end),
                Some(SegmentRangeInclusive::new(block_start, block_end)),
                None,
                StaticFileSegment::StorageChangeSets,
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
        let storage_change_sets = StorageChangeSets::new(test_case.prune_mode);
        let segments: Vec<Box<dyn Segment<_>>> = vec![Box::new(storage_change_sets)];

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
        assert_eq!(*segment, PruneSegment::StorageChangeSets);
        assert_eq!(output.pruned, test_case.expected_pruned);

        let static_provider = factory.static_file_provider();
        assert_eq!(
            static_provider.get_lowest_range_end(StaticFileSegment::StorageChangeSets),
            test_case.expected_lowest_block
        );
        assert_eq!(
            static_provider.get_highest_static_file_block(StaticFileSegment::StorageChangeSets),
            Some(tip)
        );
    }

    #[test]
    fn storage_change_sets_prune_through_pruner() {
        let factory = create_test_provider_factory();
        let tip = 2_499_999;
        setup_static_file_jars(&factory, tip);

        let (_, finished_exex_height_rx) = tokio::sync::watch::channel(FinishedExExHeight::NoExExs);

        let test_cases = vec![
            // PruneMode::Before(750_000) → deletes jar 1 (0-499_999)
            // StorageChangeSets has no tx_range, so pruned count is 0
            PruneTestCase {
                prune_mode: PruneMode::Before(750_000),
                expected_pruned: 0,
                expected_lowest_block: Some(999_999),
            },
            // PruneMode::Before(850_000) → no deletion (jar 2: 500_000-999_999 contains target)
            PruneTestCase {
                prune_mode: PruneMode::Before(850_000),
                expected_pruned: 0,
                expected_lowest_block: Some(999_999),
            },
            // PruneMode::Before(1_599_999) → deletes jars 2-3 (500_000-1_499_999)
            PruneTestCase {
                prune_mode: PruneMode::Before(1_599_999),
                expected_pruned: 0,
                expected_lowest_block: Some(1_999_999),
            },
            // PruneMode::Distance(500_000) with tip=2_499_999 → deletes jar 4
            // (1_500_000-1_999_999)
            PruneTestCase {
                prune_mode: PruneMode::Distance(500_000),
                expected_pruned: 0,
                expected_lowest_block: Some(2_499_999),
            },
            // PruneMode::Before(2_300_000) → no deletion (jar 5: 2_000_000-2_499_999 contains
            // target)
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
    fn segment_identity() {
        let mode = PruneMode::Full;
        let segment = StorageChangeSets::new(mode);

        assert_eq!(
            Segment::<ProviderFactory<MockNodeTypesWithDB>>::segment(&segment),
            PruneSegment::StorageChangeSets
        );
        assert!(Segment::<ProviderFactory<MockNodeTypesWithDB>>::purpose(&segment).is_user());
        assert_eq!(Segment::<ProviderFactory<MockNodeTypesWithDB>>::mode(&segment), Some(mode));
    }
}
