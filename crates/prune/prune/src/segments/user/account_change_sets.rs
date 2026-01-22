use crate::{
    segments::{self, PruneInput, Segment},
    PrunerError,
};
use reth_provider::{BlockReader, StaticFileProviderFactory};
use reth_prune_types::{PruneMode, PrunePurpose, PruneSegment, SegmentOutput};
use reth_static_file_types::StaticFileSegment;

/// Segment responsible for pruning account change sets in static files.
#[derive(Debug)]
pub struct AccountChangeSets {
    mode: PruneMode,
}

impl AccountChangeSets {
    /// Creates a new [`AccountChangeSets`] segment with the given prune mode.
    pub const fn new(mode: PruneMode) -> Self {
        Self { mode }
    }
}

impl<Provider> Segment<Provider> for AccountChangeSets
where
    Provider: StaticFileProviderFactory + BlockReader,
{
    fn segment(&self) -> PruneSegment {
        PruneSegment::AccountChangeSets
    }

    fn mode(&self) -> Option<PruneMode> {
        Some(self.mode)
    }

    fn purpose(&self) -> PrunePurpose {
        PrunePurpose::User
    }

    fn prune(&self, provider: &Provider, input: PruneInput) -> Result<SegmentOutput, PrunerError> {
        segments::prune_static_files(provider, input, StaticFileSegment::AccountChangeSets)
    }
}
