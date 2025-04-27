//! Contains RPC handler implementations specific to tracing.

use reth_evm::ConfigureEvm;
use reth_node_api::FullNodeComponents;
use reth_rpc_eth_api::{
    helpers::{LoadState, Trace},
    FromEvmError,
};
use reth_storage_api::BlockReader;

use crate::EthApi;

impl<Components: FullNodeComponents> Trace for EthApi<Components>
where
    Self: LoadState<Provider: BlockReader, Evm: ConfigureEvm, Error: FromEvmError<Self::Evm>>,
    Components::Provider: BlockReader,
    // <<<Self as FullNodeTypes>::Types as NodeTypes>::Primitives as NodePrimitives>::BlockHeader
    // == _
{
}
