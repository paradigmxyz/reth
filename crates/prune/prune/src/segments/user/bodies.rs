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
