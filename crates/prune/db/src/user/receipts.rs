use crate::receipts;
use reth_db_api::{table::Value, transaction::DbTxMut};
use reth_primitives_traits::NodePrimitives;
use reth_provider::{
    errors::provider::ProviderResult, BlockReader, DBProvider, NodePrimitivesProvider,
    PruneCheckpointWriter, TransactionsProvider,
};
use reth_prune::{
    segments::{PruneInput, Segment},
    PrunerError,
};
use reth_prune_types::{PruneCheckpoint, PruneMode, PrunePurpose, PruneSegment, SegmentOutput};
use tracing::instrument;

/// Responsible for pruning receipts tables.
#[derive(Debug)]
pub struct Receipts {
    mode: PruneMode,
}

impl Receipts {
    /// Creates a new receipts pruner with `mode`.
    pub const fn new(mode: PruneMode) -> Self {
        Self { mode }
    }
}

impl<Provider> Segment<Provider> for Receipts
where
    Provider: DBProvider<Tx: DbTxMut>
        + PruneCheckpointWriter
        + TransactionsProvider
        + BlockReader
        + NodePrimitivesProvider<Primitives: NodePrimitives<Receipt: Value>>,
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

    #[instrument(target = "pruner", skip(self, provider), ret(level = "trace"))]
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
