//! Contains RPC handler implementations specific to tracing.

use reth_evm::ConfigureEvmEnv;
use reth_node_api::NodePrimitives;
use reth_provider::{BlockReader, ProviderHeader, ProviderTx};
use reth_rpc_eth_api::{
    helpers::{LoadState, Trace},
    FromEvmError,
};

use crate::EthApi;

impl<Provider, Pool, Network, EvmConfig> Trace for EthApi<Provider, Pool, Network, EvmConfig>
where
    Self: LoadState<
        Provider: BlockReader,
        Evm: ConfigureEvmEnv<
            Primitives: NodePrimitives<
                BlockHeader = ProviderHeader<Self::Provider>,
                SignedTx = ProviderTx<Self::Provider>,
            >,
        >,
        Error: FromEvmError<Self::Evm>,
    >,
    Provider: BlockReader,
{
}
