use crate::receipts;
use reth_db_api::{table::Value, transaction::DbTxMut};
use reth_primitives_traits::NodePrimitives;
use reth_provider::{
    errors::provider::ProviderResult, providers::StaticFileProvider, BlockReader, DBProvider,
    PruneCheckpointWriter, StaticFileProviderFactory, TransactionsProvider,
};
use reth_prune::{
    segments::{PruneInput, Segment},
    PrunerError,
};
use reth_prune_types::{PruneCheckpoint, PruneMode, PrunePurpose, PruneSegment, SegmentOutput};
use reth_static_file_types::StaticFileSegment;

/// Responsible for pruning receipts.
#[derive(Debug)]
pub struct Receipts<N> {
    static_file_provider: StaticFileProvider<N>,
}

impl<N> Receipts<N> {
    /// Creates a receipts pruner that uses `static_file_provider`.
    pub const fn new(static_file_provider: StaticFileProvider<N>) -> Self {
        Self { static_file_provider }
    }
}

impl<Provider> Segment<Provider> for Receipts<Provider::Primitives>
where
    Provider: StaticFileProviderFactory<Primitives: NodePrimitives<Receipt: Value>>
        + DBProvider<Tx: DbTxMut>
        + PruneCheckpointWriter
        + TransactionsProvider
        + BlockReader,
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
        receipts::prune(provider, input)
    }

    fn save_checkpoint(
        &self,
        provider: &Provider,
        checkpoint: PruneCheckpoint,
    ) -> ProviderResult<()> {
        receipts::save_checkpoint(provider, checkpoint)
    }
}
