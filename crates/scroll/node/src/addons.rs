use crate::ScrollEngineValidatorBuilder;
use reth_network::NetworkHandle;
use reth_node_builder::{rpc::RpcAddOns, FullNodeComponents, FullNodeTypes};
use reth_rpc::EthApi;

/// Add-ons for the Scroll follower node.
pub type ScrollAddOns<N> = RpcAddOns<
    N,
    EthApi<
        <N as FullNodeTypes>::Provider,
        <N as FullNodeComponents>::Pool,
        NetworkHandle,
        <N as FullNodeComponents>::Evm,
    >,
    ScrollEngineValidatorBuilder,
>;
