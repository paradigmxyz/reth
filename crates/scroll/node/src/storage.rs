use reth_db::transaction::{DbTx, DbTxMut};
use reth_ethereum_forks::EthereumHardforks;
use reth_node_types::NodeTypes;
use reth_primitives::{BlockBody, EthPrimitives};
use reth_provider::{
    providers::ChainStorage, BlockBodyReader, BlockBodyWriter, ChainSpecProvider, DBProvider,
    EthStorage, ProviderResult, ReadBodyInput,
};

/// Storage implementation for Scroll.
#[derive(Debug, Default, Clone)]
pub struct ScrollStorage(EthStorage);

impl<Provider> BlockBodyWriter<Provider, BlockBody> for ScrollStorage
where
    Provider: DBProvider<Tx: DbTxMut>,
{
    fn write_block_bodies(
        &self,
        provider: &Provider,
        bodies: Vec<(u64, Option<BlockBody>)>,
    ) -> ProviderResult<()> {
        self.0.write_block_bodies(provider, bodies)
    }

    fn remove_block_bodies_above(
        &self,
        provider: &Provider,
        block: alloy_primitives::BlockNumber,
    ) -> ProviderResult<()> {
        self.0.remove_block_bodies_above(provider, block)
    }
}

impl<Provider> BlockBodyReader<Provider> for ScrollStorage
where
    Provider: DBProvider + ChainSpecProvider<ChainSpec: EthereumHardforks>,
{
    type Block = reth_primitives::Block;

    fn read_block_bodies(
        &self,
        provider: &Provider,
        inputs: Vec<ReadBodyInput<'_, Self::Block>>,
    ) -> ProviderResult<Vec<BlockBody>> {
        self.0.read_block_bodies(provider, inputs)
    }
}

impl ChainStorage<EthPrimitives> for ScrollStorage {
    fn reader<TX, Types>(
        &self,
    ) -> impl reth_provider::ChainStorageReader<reth_provider::DatabaseProvider<TX, Types>, EthPrimitives>
    where
        TX: DbTx + 'static,
        Types: reth_provider::providers::NodeTypesForProvider<Primitives = EthPrimitives>,
    {
        self
    }

    fn writer<TX, Types>(
        &self,
    ) -> impl reth_provider::ChainStorageWriter<reth_provider::DatabaseProvider<TX, Types>, EthPrimitives>
    where
        TX: DbTxMut + DbTx + 'static,
        Types: NodeTypes<Primitives = EthPrimitives>,
    {
        self
    }
}
