use alloy_consensus::Header;
use alloy_primitives::BlockNumber;
use core::marker::PhantomData;
use reth_chainspec::{ChainSpecProvider, EthChainSpec, EthereumHardforks};
use reth_db_api::{cursor::DbCursorRO, tables, transaction::DbTx};
use reth_optimism_forks::OpHardforks;
use reth_optimism_primitives::OpTransactionSigned;
use reth_primitives_traits::{Block, BlockBody, FullBlockHeader, SignedTransaction};
use reth_storage_api::{errors::ProviderResult, DBProvider, StorageLocation};

/// Trait that implements how block bodies are written to the storage.
#[auto_impl::auto_impl(&, Arc)]
pub trait BlockBodyWriter<Provider, Body: BlockBody> {
    /// Writes a set of block bodies to the storage.
    fn write_block_bodies(
        &self,
        provider: &Provider,
        bodies: Vec<(BlockNumber, Option<Body>)>,
        write_to: StorageLocation,
    );

    /// Removes all block bodies above the given block number from the database.
    fn remove_block_bodies_above(
        &self,
        _provider: &Provider,
        _block: BlockNumber,
        _remove_from: StorageLocation,
    );
}

/// Input for reading a block body. Contains a header of block being read and a list of pre-fetched
/// transactions.
pub type ReadBodyInput<'a, B> =
    (&'a <B as Block>::Header, Vec<<<B as Block>::Body as BlockBody>::Transaction>);

/// Trait that implements how block bodies are read from the storage.
///
/// Note: Within the current abstraction, transactions persistence is handled separately, thus this
/// trait is provided with transactions read beforehand and is expected to construct the block body
/// from those transactions and additional data read from elsewhere.
#[auto_impl::auto_impl(&, Arc)]
pub trait BlockBodyReader<Provider> {
    /// The block type.
    type Block: Block;

    /// Receives a list of block headers along with block transactions and returns the block bodies.
    fn read_block_bodies(
        &self,
        provider: &Provider,
        inputs: Vec<ReadBodyInput<'_, Self::Block>>,
    ) -> ProviderResult<Vec<<Self::Block as Block>::Body>>;
}

/// Optimism storage implementation.
#[derive(Debug, Clone, Copy)]
pub struct OptStorage<T = OpTransactionSigned, H = Header>(PhantomData<(T, H)>);

impl<Provider, T, H> BlockBodyWriter<Provider, alloy_consensus::BlockBody<T, H>>
    for OptStorage<T, H>
where
    T: SignedTransaction,
    H: FullBlockHeader,
{
    fn write_block_bodies(
        &self,
        _provider: &Provider,
        _bodies: Vec<(u64, Option<alloy_consensus::BlockBody<T, H>>)>,
        _write_to: StorageLocation,
    ) {
        // noop
    }

    fn remove_block_bodies_above(
        &self,
        _provider: &Provider,
        _block: BlockNumber,
        _remove_from: StorageLocation,
    ) {
        // noop
    }
}

impl<Provider, T, H> BlockBodyReader<Provider> for OptStorage<T, H>
where
    Provider: ChainSpecProvider<ChainSpec: EthChainSpec + OpHardforks> + DBProvider,
    T: SignedTransaction,
    H: FullBlockHeader,
{
    type Block = alloy_consensus::Block<T, H>;

    fn read_block_bodies(
        &self,
        provider: &Provider,
        inputs: Vec<ReadBodyInput<'_, Self::Block>>,
    ) -> ProviderResult<Vec<<Self::Block as Block>::Body>> {
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

            bodies.push(alloy_consensus::BlockBody {
                transactions,
                withdrawals,
                ..Default::default()
            });
        }

        Ok(bodies)
    }
}
