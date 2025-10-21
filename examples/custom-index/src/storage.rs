use reth_ethereum::{
    chainspec::EthereumHardforks,
    evm::revm::primitives::alloy_primitives::BlockNumber,
    primitives::Header,
    provider::{db::transaction::DbTxMut, ChainSpecProvider, ProviderResult},
    storage::{ChainStorageReader, ChainStorageWriter, DBProvider, EthStorage},
    BlockBody, EthPrimitives, TransactionSigned,
};

/// Custom storage implementation.
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct CustomStorage {
    inner: EthStorage,
}

impl<Provider> ChainStorageReader<Provider, EthPrimitives> for CustomStorage
where
    Provider: DBProvider + ChainSpecProvider<ChainSpec: EthereumHardforks>,
{
    fn read_block_bodies(
        &self,
        provider: &Provider,
        inputs: Vec<(&Header, Vec<TransactionSigned>)>,
    ) -> ProviderResult<Vec<BlockBody>> {
        ChainStorageReader::<_, EthPrimitives>::read_block_bodies(&self.inner, provider, inputs)
    }
}

impl<Provider> ChainStorageWriter<Provider, EthPrimitives> for CustomStorage
where
    Provider: DBProvider<Tx: DbTxMut> + ChainSpecProvider<ChainSpec: EthereumHardforks>,
{
    fn write_block_bodies(
        &self,
        provider: &Provider,
        bodies: Vec<(u64, Option<BlockBody>)>,
    ) -> ProviderResult<()> {
        ChainStorageWriter::<_, EthPrimitives>::write_block_bodies(&self.inner, provider, bodies)
    }

    fn remove_block_bodies_above(
        &self,
        provider: &Provider,
        block: BlockNumber,
    ) -> ProviderResult<()> {
        ChainStorageWriter::<_, EthPrimitives>::remove_block_bodies_above(
            &self.inner,
            provider,
            block,
        )
    }
}
