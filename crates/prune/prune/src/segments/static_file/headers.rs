use crate::{
    segments::{PruneInput, PruneOutput, Segment},
    PrunerError,
};
use reth_db::database::Database;
use reth_provider::{providers::StaticFileProvider, DatabaseProviderRW};
use reth_prune_types::{PruneMode, PrunePurpose, PruneSegment};
use reth_static_file_types::StaticFileSegment;

#[derive(Debug)]
pub struct Headers {
    static_file_provider: StaticFileProvider,
}

impl Headers {
    pub const fn new(static_file_provider: StaticFileProvider) -> Self {
        Self { static_file_provider }
    }
}

impl<DB: Database> Segment<DB> for Headers {
    fn segment(&self) -> PruneSegment {
        PruneSegment::Headers
    }

    fn mode(&self) -> Option<PruneMode> {
        self.static_file_provider
            .get_highest_static_file_block(StaticFileSegment::Headers)
            .map(PruneMode::before_inclusive)
    }

    fn purpose(&self) -> PrunePurpose {
        PrunePurpose::StaticFile
    }

    fn prune(
        &self,
        provider: &DatabaseProviderRW<DB>,
        input: PruneInput,
    ) -> Result<PruneOutput, PrunerError> {
        crate::segments::headers::prune(provider, input)
    }
}
