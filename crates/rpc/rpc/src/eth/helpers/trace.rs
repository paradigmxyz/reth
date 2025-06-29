//! Contains RPC handler implementations specific to tracing.

use reth_evm::ConfigureEvm;
use reth_node_api::NodePrimitives;
use reth_rpc_eth_api::{
    helpers::{LoadState, Trace},
    FromEvmError, RpcNodeCore,
};
use reth_storage_api::{BlockReader, ProviderHeader, ProviderTx};

use crate::EthApi;

impl<N> Trace for EthApi<N>
where
    Self: LoadState<
        Provider: BlockReader,
        Evm: ConfigureEvm<
            Primitives: NodePrimitives<
                BlockHeader = ProviderHeader<Self::Provider>,
                SignedTx = ProviderTx<Self::Provider>,
            >,
        >,
        Error: FromEvmError<Self::Evm>,
    >,
    N: RpcNodeCore<Provider: BlockReader>,
{
}
