use crate::{
    segments::{PruneInput, PruneOutput, Segment},
    PrunerError,
};
use reth_db_api::database::Database;
use reth_provider::DatabaseProviderRW;
use reth_prune_types::{PruneMode, PrunePurpose, PruneSegment};
use tracing::instrument;

#[derive(Debug)]
pub struct Transactions {
    mode: PruneMode,
}

impl Transactions {
    pub const fn new(mode: PruneMode) -> Self {
        Self { mode }
    }
}

impl<DB: Database> Segment<DB> for Transactions {
    fn segment(&self) -> PruneSegment {
        PruneSegment::Transactions
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
    ) -> Result<PruneOutput, PrunerError> {
        crate::segments::transactions::prune(provider, input)
    }
}
