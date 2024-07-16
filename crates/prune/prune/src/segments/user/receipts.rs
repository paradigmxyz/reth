use crate::{
    segments::{PruneInput, Segment},
    PrunerError,
};
use reth_db_api::database::Database;
use reth_provider::{errors::provider::ProviderResult, DatabaseProviderRW};
use reth_prune_types::{PruneCheckpoint, PruneMode, PrunePurpose, PruneSegment, SegmentOutput};
use tracing::instrument;

#[derive(Debug)]
pub struct Receipts {
    mode: PruneMode,
}

impl Receipts {
    pub const fn new(mode: PruneMode) -> Self {
        Self { mode }
    }
}

impl<DB: Database> Segment<DB> for Receipts {
    fn segment(&self) -> PruneSegment {
        PruneSegment::Receipts
    }

    fn mode(&self) -> Option<PruneMode> {
        Some(self.mode)
    }

    fn purpose(&self) -> PrunePurpose {
        PrunePurpose::User
    }

    #[instrument(level = "trace", target = "pruner", skip(self, provider), ret)]
    fn prune(
        &self,
        provider: &DatabaseProviderRW<DB>,
        input: PruneInput,
    ) -> Result<SegmentOutput, PrunerError> {
        crate::segments::receipts::prune(provider, input)
    }

    fn save_checkpoint(
        &self,
        provider: &DatabaseProviderRW<DB>,
        checkpoint: PruneCheckpoint,
    ) -> ProviderResult<()> {
        crate::segments::receipts::save_checkpoint(provider, checkpoint)
    }
}
