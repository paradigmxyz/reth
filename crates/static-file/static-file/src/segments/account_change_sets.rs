//! Account changeset static files

use crate::segments::Segment;
use alloy_primitives::BlockNumber;
use reth_primitives_traits::NodePrimitives;
use reth_provider::{
    providers::StaticFileWriter, BlockReader, ChangeSetReader, DBProvider,
    StaticFileProviderFactory,
};
use reth_static_file_types::StaticFileSegment;
use reth_storage_errors::provider::ProviderResult;
use std::ops::RangeInclusive;

/// Static File segment responsible for [`StaticFileSegment::AccountChangeSets`] part of data.
#[derive(Debug, Default)]
pub struct AccountChangeSets;

impl<Provider> Segment<Provider> for AccountChangeSets
where
    Provider: StaticFileProviderFactory<Primitives: NodePrimitives>
        + DBProvider
        + BlockReader
        + ChangeSetReader,
{
    fn segment(&self) -> StaticFileSegment {
        StaticFileSegment::AccountChangeSets
    }

    fn copy_to_static_files(
        &self,
        provider: Provider,
        block_range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<()> {
        let static_file_provider = provider.static_file_provider();
        let mut static_file_writer = static_file_provider
            .get_writer(*block_range.start(), StaticFileSegment::AccountChangeSets)?;

        for block in block_range {
            static_file_writer.increment_block(block)?;

            let account_changeset = provider.account_block_changeset(block)?;
            static_file_writer.append_account_changeset(account_changeset)?;
        }

        Ok(())
    }
}
