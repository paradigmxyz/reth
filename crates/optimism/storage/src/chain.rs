use alloy_consensus::Header;
use alloy_primitives::BlockNumber;
use core::marker::PhantomData;
use reth_chainspec::{ChainSpecProvider, EthChainSpec, EthereumHardforks};
use reth_optimism_forks::OpHardforks;
use reth_optimism_primitives::OpTransactionSigned;
use reth_primitives_traits::{Block, FullBlockHeader, SignedTransaction};
use reth_storage_api::{
    errors::{ProviderError, ProviderResult},
    BlockBodyReader, BlockBodyWriter, DBProvider, ReadBodyInput, StorageLocation,
};

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
    ) -> ProviderResult<()> {
        // noop
        Ok(())
    }

    fn remove_block_bodies_above(
        &self,
        _provider: &Provider,
        _block: BlockNumber,
        _remove_from: StorageLocation,
    ) -> ProviderResult<()> {
        // noop
        Ok(())
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
        let chain_spec = provider.chain_spec();

        for (header, _) in inputs {
            // If we are past shanghai, then all blocks should have a withdrawal list,
            // even if empty
            if !chain_spec.is_shanghai_active_at_timestamp(header.timestamp()) {
                return Err(ProviderError::InvalidStorageOutput);
            }
        }

        Ok(vec![])
    }
}
