use alloy_consensus::TxEnvelope;
use alloy_network::eip2718::Decodable2718;
use alloy_primitives::{Bytes, B256};
use reth::{
    builder::{rpc::RpcRegistry, FullNodeComponents},
    rpc::api::{
        eth::helpers::{EthApiSpec, EthTransactions, TraceExt},
        DebugApiServer,
    },
};
use reth_chainspec::EthereumHardforks;
use reth_node_builder::{EthApiTypes, NodeTypes};

#[allow(missing_debug_implementations)]
pub struct RpcTestContext<Node: FullNodeComponents, EthApi: EthApiTypes> {
    pub inner: RpcRegistry<Node, EthApi>,
}

impl<Node, EthApi> RpcTestContext<Node, EthApi>
where
    Node: FullNodeComponents<Types: NodeTypes<ChainSpec: EthereumHardforks>>,
    EthApi: EthApiSpec + EthTransactions + TraceExt,
{
    /// Injects a raw transaction into the node tx pool via RPC server
    pub async fn inject_tx(&self, raw_tx: Bytes) -> Result<B256, EthApi::Error> {
        let eth_api = self.inner.eth_api();
        eth_api.send_raw_transaction(raw_tx).await
    }

    /// Retrieves a transaction envelope by its hash
    pub async fn envelope_by_hash(&self, hash: B256) -> eyre::Result<TxEnvelope> {
        let tx = self.inner.debug_api().raw_transaction(hash).await?.unwrap();
        let tx = tx.to_vec();
        Ok(TxEnvelope::decode_2718(&mut tx.as_ref()).unwrap())
    }
}
