use alloy_consensus::TxEnvelope;
use alloy_network::eip2718::Decodable2718;
use reth::{api::FullNodeComponents, builder::rpc::RpcRegistry, rpc::api::DebugApiServer};
use reth_primitives::{Bytes, B256};
use reth_rpc::eth::{error::EthResult, EthTransactions};

pub struct RpcTestContext<Node: FullNodeComponents> {
    pub inner: RpcRegistry<Node>,
}

impl<Node: FullNodeComponents> RpcTestContext<Node> {
    /// Injects a raw transaction into the node tx pool via RPC server
    pub async fn inject_tx(&mut self, raw_tx: Bytes) -> EthResult<B256> {
        let eth_api = self.inner.eth_api();
        eth_api.send_raw_transaction(raw_tx).await
    }

    /// Retrieves a transaction envelope by its hash
    pub async fn envelope_by_hash(&mut self, hash: B256) -> eyre::Result<TxEnvelope> {
        let tx = self.inner.debug_api().raw_transaction(hash).await?.unwrap();
        let tx = tx.to_vec();
        Ok(TxEnvelope::decode_2718(&mut tx.as_ref()).unwrap())
    }
}
