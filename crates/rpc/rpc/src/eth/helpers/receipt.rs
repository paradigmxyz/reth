//! Builds an RPC receipt response w.r.t. data layout of network.

use crate::EthApi;
use alloy_consensus::crypto::RecoveryError;
use reth_chainspec::ChainSpecProvider;
use reth_node_api::NodePrimitives;
use reth_rpc_convert::RpcConvert;
use reth_rpc_eth_api::{helpers::LoadReceipt, EthApiTypes, RpcNodeCoreExt};
use reth_storage_api::{BlockReader, ProviderReceipt, ProviderTx};

impl<Provider, Pool, Network, EvmConfig, Rpc> LoadReceipt
    for EthApi<Provider, Pool, Network, EvmConfig, Rpc>
where
    Self: RpcNodeCoreExt<
            Primitives: NodePrimitives<
                SignedTx = ProviderTx<Self::Provider>,
                Receipt = ProviderReceipt<Self::Provider>,
            >,
        > + EthApiTypes<
            NetworkTypes = Rpc::Network,
            RpcConvert: RpcConvert<
                Network = Rpc::Network,
                Primitives = Self::Primitives,
                Error = Self::Error,
            >,
            Error: From<RecoveryError>,
        >,
    Provider: BlockReader + ChainSpecProvider,
    Rpc: RpcConvert,
{
}
