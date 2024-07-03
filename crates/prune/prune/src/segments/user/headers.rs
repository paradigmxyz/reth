use reth_db::database::Database;
use reth_provider::DatabaseProviderRW;
use reth_prune_types::{PruneMode, PrunePurpose, PruneSegment};
use tracing::instrument;

use crate::{
    segments::{PruneInput, PruneOutput, Segment},
    PrunerError,
};

#[derive(Debug)]
pub struct Headers {
    mode: PruneMode,
}

impl Headers {
    pub const fn new(mode: PruneMode) -> Self {
        Self { mode }
    }
}

impl<DB: Database> Segment<DB> for Headers {
    fn segment(&self) -> PruneSegment {
        PruneSegment::Headers
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
        crate::segments::headers::prune(provider, input)
    }
}
