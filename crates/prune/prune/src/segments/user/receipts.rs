use crate::{
    segments::{self, PruneInput, Segment},
    PrunerError,
};
use reth_db_api::{table::Value, transaction::DbTxMut};
use reth_primitives_traits::NodePrimitives;
use reth_provider::{
    errors::provider::ProviderResult, BlockReader, DBProvider, EitherWriter,
    NodePrimitivesProvider, PruneCheckpointWriter, StaticFileProviderFactory, StorageSettingsCache,
    TransactionsProvider,
};
use reth_prune_types::{PruneCheckpoint, PruneMode, PrunePurpose, PruneSegment, SegmentOutput};
use reth_static_file_types::StaticFileSegment;
use tracing::{debug, instrument};

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
    Provider: DBProvider<Tx: DbTxMut>
        + PruneCheckpointWriter
        + TransactionsProvider
        + BlockReader
        + StorageSettingsCache
        + StaticFileProviderFactory
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

    #[instrument(
        name = "Receipts::prune",
        target = "pruner",
        skip(self, provider),
        ret(level = "trace")
    )]
    fn prune(&self, provider: &Provider, input: PruneInput) -> Result<SegmentOutput, PrunerError> {
        if EitherWriter::receipts_destination(provider).is_static_file() && self.mode.is_full() {
            debug!(target: "pruner", "PruneMode::Full: deleting all receipts static files.");
            return segments::delete_static_files_segment(
                provider,
                input,
                StaticFileSegment::Receipts,
            )
        }

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
