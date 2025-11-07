use alloy_consensus::{EthereumTxEnvelope, TxEip4844Variant};
use alloy_eips::eip7594::BlobTransactionSidecarVariant;
use alloy_network::eip2718::Decodable2718;
use alloy_primitives::{Bytes, B256};
use reth_chainspec::EthereumHardforks;
use reth_node_api::{BlockTy, FullNodeComponents};
use reth_node_builder::{rpc::RpcRegistry, NodeTypes};
use reth_provider::BlockReader;
use reth_rpc_api::DebugApiServer;
use reth_rpc_eth_api::{
    helpers::{EthApiSpec, EthTransactions, TraceExt},
    EthApiTypes,
};

#[expect(missing_debug_implementations)]
pub struct RpcTestContext<Node: FullNodeComponents, EthApi: EthApiTypes> {
    pub inner: RpcRegistry<Node, EthApi>,
}

impl<Node, EthApi> RpcTestContext<Node, EthApi>
where
    Node: FullNodeComponents<Types: NodeTypes<ChainSpec: EthereumHardforks>>,
    EthApi: EthApiSpec<Provider: BlockReader<Block = BlockTy<Node::Types>>>
        + EthTransactions
        + TraceExt
        + reth_rpc_eth_api::helpers::LegacyRpc,
{
    /// Injects a raw transaction into the node tx pool via RPC server
    pub async fn inject_tx(&self, raw_tx: Bytes) -> Result<B256, EthApi::Error> {
        let eth_api = self.inner.eth_api();
        eth_api.send_raw_transaction(raw_tx).await
    }

    /// Retrieves a transaction envelope by its hash
    pub async fn envelope_by_hash(
        &self,
        hash: B256,
    ) -> eyre::Result<EthereumTxEnvelope<TxEip4844Variant<BlobTransactionSidecarVariant>>> {
        let tx = self.inner.debug_api().raw_transaction(hash).await?.unwrap();
        let tx = tx.to_vec();
        Ok(EthereumTxEnvelope::decode_2718(&mut tx.as_ref()).unwrap())
    }
}
