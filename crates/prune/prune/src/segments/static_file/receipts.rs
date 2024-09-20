use crate::{
    segments::{PruneInput, Segment},
    PrunerError,
};
use reth_db::transaction::DbTxMut;
use reth_provider::{
    errors::provider::ProviderResult, providers::StaticFileProvider, BlockReader, DBProvider,
    PruneCheckpointWriter, TransactionsProvider,
};
use reth_prune_types::{PruneCheckpoint, PruneMode, PrunePurpose, PruneSegment, SegmentOutput};
use reth_static_file_types::StaticFileSegment;

#[derive(Debug)]
pub struct Receipts {
    static_file_provider: StaticFileProvider,
}

impl Receipts {
    pub const fn new(static_file_provider: StaticFileProvider) -> Self {
        Self { static_file_provider }
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
        self.static_file_provider
            .get_highest_static_file_block(StaticFileSegment::Receipts)
            .map(PruneMode::before_inclusive)
    }

    fn purpose(&self) -> PrunePurpose {
        PrunePurpose::StaticFile
    }

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
