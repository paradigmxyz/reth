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
        + TraceExt,
{
    /// Injects a raw transaction into the node tx pool via RPC server
    pub async fn inject_tx(&self, raw_tx: Bytes) -> Result<B256, EthApi::Error> {
        let eth_api = self.inner.eth_api();
        eth_api.send_raw_transaction(raw_tx).await
    }

    /// Retrieves an Ethereum transaction envelope by its hash, including its blob sidecar if the
    /// transaction is still pooled.
    pub async fn envelope_by_hash(
        &self,
        hash: B256,
    ) -> eyre::Result<EthereumTxEnvelope<TxEip4844Variant<BlobTransactionSidecarVariant>>> {
        self.decoded_transaction_by_hash(hash).await
    }

    /// Retrieves the raw transaction with the given hash via `debug_getRawTransaction` and decodes
    /// it as `T`.
    ///
    /// Returns an error if the node does not know the transaction.
    pub async fn decoded_transaction_by_hash<T: Decodable2718>(
        &self,
        hash: B256,
    ) -> eyre::Result<T> {
        let tx = self
            .inner
            .debug_api()
            .raw_transaction(hash)
            .await?
            .ok_or_else(|| eyre::eyre!("transaction {hash} not found"))?;
        Ok(T::decode_2718_exact(&tx)?)
    }
}
