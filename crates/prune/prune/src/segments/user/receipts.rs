use crate::{
    segments::{PruneInput, Segment},
    PrunerError,
};
use reth_db::transaction::DbTxMut;
use reth_provider::{
    errors::provider::ProviderResult, BlockReader, DBProvider, PruneCheckpointWriter,
    TransactionsProvider,
};
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

impl<Provider> Segment<Provider> for Receipts
where
    Provider: DBProvider<Tx: DbTxMut> + PruneCheckpointWriter + TransactionsProvider + BlockReader,
{
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
    fn prune(&self, provider: &Provider, input: PruneInput) -> Result<SegmentOutput, PrunerError> {
        crate::segments::receipts::prune(provider, input)
    }

    fn save_checkpoint(
        &self,
        provider: &Provider,
        checkpoint: PruneCheckpoint,
    ) -> ProviderResult<()> {
        crate::segments::receipts::save_checkpoint(provider, checkpoint)
    }
}
