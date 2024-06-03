use crate::ProviderFactory;
use reth_db::{
    cursor::DbCursorRO,
    database::Database,
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_errors::ProviderResult;
use reth_primitives::BlockNumber;
use std::collections::BTreeMap;

/// Table key used to read and write the last finalized block.
const FINALIZED_BLOCKS_KEY: u8 = 0;

/// Methods to read/write the last known finalized block from the database.
/// The default implementation already has all the required functionality,
/// only the `provider_factory` method needs to be implemented to wire the
/// functionality with the consumer's provider.
pub trait FinalizedBlockProvider<DB>
where
    DB: Database,
{
    /// Fetches and returns the latest finalized block number.
    fn fetch_latest_finalized_block_number(&self) -> ProviderResult<BlockNumber> {
        let mut finalized_blocks = self
            .provider_factory()
            .provider()?
            .tx_ref()
            .cursor_read::<tables::FinalizedBlocks>()?
            .walk(Some(FINALIZED_BLOCKS_KEY))?
            .take(1)
            .collect::<Result<BTreeMap<u8, BlockNumber>, _>>()?;

        let last_finalized_block_number = finalized_blocks.pop_first().unwrap_or_default();
        Ok(last_finalized_block_number.1)
    }

    /// Saves finalized block.
    fn save_finalized_block_number(&self, block_number: BlockNumber) -> ProviderResult<()> {
        let provider = self.provider_factory().provider_rw()?;
        provider.tx_ref().put::<tables::FinalizedBlocks>(FINALIZED_BLOCKS_KEY, block_number)?;
        provider.commit()?;
        Ok(())
    }

    /// Provider factory getter.
    fn provider_factory(&self) -> &ProviderFactory<DB>;
}
