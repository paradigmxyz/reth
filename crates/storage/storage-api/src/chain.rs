use crate::DBProvider;
use alloc::vec::Vec;
use alloy_consensus::BlockHeader;
use alloy_primitives::BlockNumber;
use reth_chainspec::{ChainSpecProvider, EthereumHardforks};
use reth_db_api::{
    cursor::{DbCursorRO, DbCursorRW},
    models::StoredBlockOmmers,
    tables,
    transaction::{DbTx, DbTxMut},
    DbTxUnwindExt,
};
use reth_db_models::StoredBlockWithdrawals;
use reth_execution_types::ExecutionOutcome;
use reth_primitives_traits::{FullBlockHeader, NodePrimitives};
use reth_storage_errors::provider::ProviderResult;

/// Trait that implements how chain-specific types are written to the storage.
#[auto_impl::auto_impl(&, Arc)]
pub trait ChainStorageWriter<Provider, N: NodePrimitives> {
    /// Writes a set of block bodies to the storage.
    ///
    /// Note: Within the current abstraction, this should only write to tables unrelated to
    /// transactions. Writing of transactions is handled separately.
    fn write_block_bodies(
        &self,
        provider: &Provider,
        bodies: Vec<(BlockNumber, Option<N::BlockBody>)>,
    ) -> ProviderResult<()>;

    /// Removes all block bodies above the given block number from the database.
    fn remove_block_bodies_above(
        &self,
        provider: &Provider,
        block: BlockNumber,
    ) -> ProviderResult<()>;

    /// Hook to write any custom indices or state that is not covered by other methods.
    fn write_custom_state(
        &self,
        _provider: &Provider,
        _state: &ExecutionOutcome<N::Receipt>,
    ) -> ProviderResult<()> {
        Ok(())
    }

    /// Hook to revert any [`ChainStorageWriter::write_custom_state`] changes.
    fn remove_custom_state_above(
        &self,
        _provider: &Provider,
        _block: BlockNumber,
    ) -> ProviderResult<()> {
        Ok(())
    }
}

/// Trait that implements how chain-specific types are read from storage.
#[auto_impl::auto_impl(&, Arc)]
pub trait ChainStorageReader<Provider, N: NodePrimitives> {
    /// Receives a list of block headers along with block transactions and returns the block bodies.
    fn read_block_bodies(
        &self,
        provider: &Provider,
        inputs: Vec<(&N::BlockHeader, Vec<N::SignedTx>)>,
    ) -> ProviderResult<Vec<N::BlockBody>>;
}

/// Ethereum storage implementation.
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct EthStorage;

impl<Provider, N, T, H> ChainStorageWriter<Provider, N> for EthStorage
where
    Provider: DBProvider<Tx: DbTxMut>,
    H: FullBlockHeader,
    N: NodePrimitives<SignedTx = T, BlockHeader = H, BlockBody = alloy_consensus::BlockBody<T, H>>,
{
    fn write_block_bodies(
        &self,
        provider: &Provider,
        bodies: Vec<(u64, Option<alloy_consensus::BlockBody<T, H>>)>,
    ) -> ProviderResult<()> {
        let mut ommers_cursor = provider.tx_ref().cursor_write::<tables::BlockOmmers<H>>()?;
        let mut withdrawals_cursor =
            provider.tx_ref().cursor_write::<tables::BlockWithdrawals>()?;

        for (block_number, body) in bodies {
            let Some(body) = body else { continue };

            // Write ommers if any
            if !body.ommers.is_empty() {
                ommers_cursor.append(block_number, &StoredBlockOmmers { ommers: body.ommers })?;
            }

            // Write withdrawals if any
            if let Some(withdrawals) = body.withdrawals &&
                !withdrawals.is_empty()
            {
                withdrawals_cursor.append(block_number, &StoredBlockWithdrawals { withdrawals })?;
            }
        }

        Ok(())
    }

    fn remove_block_bodies_above(
        &self,
        provider: &Provider,
        block: BlockNumber,
    ) -> ProviderResult<()> {
        provider.tx_ref().unwind_table_by_num::<tables::BlockWithdrawals>(block)?;
        provider.tx_ref().unwind_table_by_num::<tables::BlockOmmers<H>>(block)?;

        Ok(())
    }
}

impl<Provider, N, T, H> ChainStorageReader<Provider, N> for EthStorage
where
    Provider: DBProvider + ChainSpecProvider<ChainSpec: EthereumHardforks>,
    H: FullBlockHeader,
    N: NodePrimitives<SignedTx = T, BlockHeader = H, BlockBody = alloy_consensus::BlockBody<T, H>>,
{
    fn read_block_bodies(
        &self,
        provider: &Provider,
        inputs: Vec<(&N::BlockHeader, Vec<N::SignedTx>)>,
    ) -> ProviderResult<Vec<N::BlockBody>> {
        // TODO: Ideally storage should hold its own copy of chain spec
        let chain_spec = provider.chain_spec();

        let mut withdrawals_cursor = provider.tx_ref().cursor_read::<tables::BlockWithdrawals>()?;

        let mut bodies = Vec::with_capacity(inputs.len());

        for (header, transactions) in inputs {
            // If we are past shanghai, then all blocks should have a withdrawal list,
            // even if empty
            let withdrawals = if chain_spec.is_shanghai_active_at_timestamp(header.timestamp()) {
                withdrawals_cursor
                    .seek_exact(header.number())?
                    .map(|(_, w)| w.withdrawals)
                    .unwrap_or_default()
                    .into()
            } else {
                None
            };
            let ommers = if chain_spec.is_paris_active_at_block(header.number()) {
                Vec::new()
            } else {
                // Pre-merge: fetch ommers from database using direct database access
                provider
                    .tx_ref()
                    .cursor_read::<tables::BlockOmmers<H>>()?
                    .seek_exact(header.number())?
                    .map(|(_, stored_ommers)| stored_ommers.ommers)
                    .unwrap_or_default()
            };
            bodies.push(alloy_consensus::BlockBody { transactions, ommers, withdrawals });
        }

        Ok(bodies)
    }
}

/// A noop storage for chains that don’t have custom body storage.
///
/// This will never read nor write additional body content such as withdrawals or ommers.
/// But will respect the optionality of withdrawals if activated and fill them if the corresponding
/// hardfork is activated.
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct EmptyBodyStorage;

impl<Provider, N: NodePrimitives> ChainStorageWriter<Provider, N> for EmptyBodyStorage {
    fn write_block_bodies(
        &self,
        _provider: &Provider,
        _bodies: Vec<(BlockNumber, Option<N::BlockBody>)>,
    ) -> ProviderResult<()> {
        // noop
        Ok(())
    }

    fn remove_block_bodies_above(
        &self,
        _provider: &Provider,
        _block: BlockNumber,
    ) -> ProviderResult<()> {
        // noop
        Ok(())
    }
}

impl<Provider, T, H, N> ChainStorageReader<Provider, N> for EmptyBodyStorage
where
    Provider: ChainSpecProvider<ChainSpec: EthereumHardforks>,
    H: BlockHeader,
    N: NodePrimitives<SignedTx = T, BlockHeader = H, BlockBody = alloy_consensus::BlockBody<T, H>>,
{
    fn read_block_bodies(
        &self,
        provider: &Provider,
        inputs: Vec<(&N::BlockHeader, Vec<N::SignedTx>)>,
    ) -> ProviderResult<Vec<N::BlockBody>> {
        let chain_spec = provider.chain_spec();

        Ok(inputs
            .into_iter()
            .map(|(header, transactions)| {
                alloy_consensus::BlockBody {
                    transactions,
                    ommers: vec![], // Empty storage never has ommers
                    withdrawals: chain_spec
                        .is_shanghai_active_at_timestamp(header.timestamp())
                        .then(Default::default),
                }
            })
            .collect())
    }
}
