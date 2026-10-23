use alloy_consensus::BlockHeader;
use reth_chainspec::{ChainSpecProvider, EthereumHardforks};
use reth_db_api::transaction::{DbTx, DbTxMut};
use reth_errors::{ProviderError, ProviderResult};
use reth_ethereum_primitives::TransactionSigned;
use reth_primitives_traits::BlockBody;
use reth_provider::{
    providers::{ChainStorage, NodeTypesForProvider},
    DatabaseProvider,
};
use reth_pureth_block::{ExecutionBlock, ExecutionBody, ExecutionHeader, ExecutionPrimitives};
use reth_storage_api::{
    BlockBodyReader, BlockBodyWriter, ChainStorageReader, ChainStorageWriter, DBProvider,
    EthStorage, ReadBodyInput,
};

#[derive(Clone, Debug, Default)]
pub struct ExecutionStorage;

impl ChainStorage<ExecutionPrimitives> for ExecutionStorage {
    fn reader<TX, Types>(
        &self,
    ) -> impl ChainStorageReader<DatabaseProvider<TX, Types>, ExecutionPrimitives>
    where
        TX: DbTx + 'static,
        Types: NodeTypesForProvider<Primitives = ExecutionPrimitives>,
    {
        self
    }

    fn writer<TX, Types>(
        &self,
    ) -> impl ChainStorageWriter<DatabaseProvider<TX, Types>, ExecutionPrimitives>
    where
        TX: DbTxMut + DbTx + 'static,
        Types: NodeTypesForProvider<Primitives = ExecutionPrimitives>,
    {
        self
    }
}

impl<Provider> BlockBodyReader<Provider> for ExecutionStorage
where
    Provider: DBProvider + ChainSpecProvider<ChainSpec: EthereumHardforks>,
{
    type Block = ExecutionBlock;

    fn read_block_bodies(
        &self,
        provider: &Provider,
        inputs: Vec<ReadBodyInput<'_, Self::Block>>,
    ) -> ProviderResult<Vec<ExecutionBody>> {
        let spec = provider.chain_spec();
        if inputs.iter().any(|(header, _)| {
            header.is_ssz() != spec.is_amsterdam_active_at_timestamp(header.timestamp())
        }) {
            return Err(ProviderError::other(
                reth_pureth_block::PayloadSourceError::UnsupportedFork,
            ));
        }
        let headers = inputs.iter().map(|(header, _)| *header).collect::<Vec<_>>();
        EthStorage::<TransactionSigned, ExecutionHeader>::default()
            .read_block_bodies(provider, inputs)?
            .into_iter()
            .zip(headers)
            .map(|(body, header)| {
                ExecutionBody::from_header(header, body).map_err(ProviderError::other)
            })
            .collect()
    }
}

impl<Provider> BlockBodyWriter<Provider, ExecutionBody> for ExecutionStorage
where
    Provider: DBProvider<Tx: DbTxMut>,
{
    fn write_block_bodies(
        &self,
        provider: &Provider,
        bodies: Vec<(u64, Option<&ExecutionBody>)>,
    ) -> ProviderResult<()> {
        let bodies = bodies
            .into_iter()
            .map(|(number, body)| {
                (
                    number,
                    body.map(|body| alloy_consensus::BlockBody {
                        transactions: Vec::<TransactionSigned>::new(),
                        ommers: body.ommers().unwrap_or_default().to_vec(),
                        withdrawals: body.withdrawals().cloned(),
                    }),
                )
            })
            .collect::<Vec<_>>();
        EthStorage::<TransactionSigned, ExecutionHeader>::default().write_block_bodies(
            provider,
            bodies.iter().map(|(number, body)| (*number, body.as_ref())).collect(),
        )
    }

    fn remove_block_bodies_above(&self, provider: &Provider, block: u64) -> ProviderResult<()> {
        EthStorage::<TransactionSigned, ExecutionHeader>::default()
            .remove_block_bodies_above(provider, block)
    }
}
