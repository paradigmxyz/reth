use crate::{
    engine_api::EngineApiTestContext, network::NetworkTestContext, payload::PayloadTestContext,
};
use alloy_consensus::{TxEip4844Variant, TxEnvelope};
use alloy_network::eip2718::Decodable2718;
use alloy_rpc_types::BlockNumberOrTag;
use eyre::Ok;
use reth::{
    api::FullNodeComponents,
    builder::FullNode,
    providers::{BlockReaderIdExt, CanonStateSubscriptions},
    rpc::{
        api::DebugApiServer,
        eth::{error::EthResult, EthTransactions},
    },
};
use reth_node_ethereum::EthEngineTypes;
use reth_primitives::{constants::eip4844::MAINNET_KZG_TRUSTED_SETUP, Bytes, B256};

use tokio_stream::StreamExt;

/// An helper struct to handle node actions
pub struct NodeTestContext<Node>
where
    Node: FullNodeComponents<Engine = EthEngineTypes>,
{
    pub inner: FullNode<Node>,
    payload: PayloadTestContext<Node::Engine>,
    pub network: NetworkTestContext,
    pub engine_api: EngineApiTestContext,
}

impl<Node> NodeTestContext<Node>
where
    Node: FullNodeComponents<Engine = EthEngineTypes>,
{
    /// Creates a new test node
    pub async fn new(node: FullNode<Node>) -> eyre::Result<Self> {
        let builder = node.payload_builder.clone();

        Ok(Self {
            inner: node.clone(),
            network: NetworkTestContext::new(node.network.clone()),
            payload: PayloadTestContext::new(builder).await?,
            engine_api: EngineApiTestContext {
                engine_api_client: node.auth_server_handle().http_client(),
                canonical_stream: node.provider.canonical_state_stream(),
            },
        })
    }

    /// Advances the node forward
    pub async fn advance(&mut self, versioned_hashes: Vec<B256>) -> eyre::Result<B256> {
        // trigger new payload building draining the pool
        let eth_attr = self.payload.new_payload().await.unwrap();

        // first event is the payload attributes
        self.payload.expect_attr_event(eth_attr.clone()).await?;

        // wait for the payload builder to have finished building
        self.payload.wait_for_built_payload(eth_attr.payload_id()).await;

        // trigger resolve payload via engine api
        self.engine_api.get_payload_v3(eth_attr.payload_id()).await?;

        // ensure we're also receiving the built payload as event
        let payload = self.payload.expect_built_payload().await?;

        // submit payload via engine api
        let block_hash =
            self.engine_api.submit_payload(payload, eth_attr.clone(), versioned_hashes).await?;

        // trigger forkchoice update via engine api to commit the block to the blockchain
        self.engine_api.update_forkchoice(block_hash).await?;

        Ok(block_hash)
    }

    /// Injects a raw transaction into the node tx pool via RPC server
    pub async fn inject_tx(&mut self, raw_tx: Bytes) -> EthResult<B256> {
        let eth_api = self.inner.rpc_registry.eth_api();
        eth_api.send_raw_transaction(raw_tx).await
    }

    pub async fn envelope_by_hash(&mut self, hash: B256) -> eyre::Result<TxEnvelope> {
        let tx = self.inner.rpc_registry.debug_api().raw_transaction(hash).await?.unwrap();
        let tx = tx.to_vec();
        Ok(TxEnvelope::decode_2718(&mut tx.as_ref()).unwrap())
    }
    pub fn validate_sidecar(&self, tx: TxEnvelope) -> Vec<B256> {
        let proof_setting = MAINNET_KZG_TRUSTED_SETUP.clone();

        match tx {
            TxEnvelope::Eip4844(signed) => match signed.tx() {
                TxEip4844Variant::TxEip4844WithSidecar(tx) => {
                    tx.validate_blob(&proof_setting).unwrap();
                    tx.sidecar.versioned_hashes().collect()
                }
                _ => panic!("Expected Eip4844 transaction with sidecar"),
            },
            _ => panic!("Expected Eip4844 transaction"),
        }
    }

    /// Asserts that a new block has been added to the blockchain
    /// and the tx has been included in the block
    pub async fn assert_new_block(
        &mut self,
        tip_tx_hash: B256,
        block_hash: B256,
    ) -> eyre::Result<()> {
        // get head block from notifications stream and verify the tx has been pushed to the
        // pool is actually present in the canonical block
        let head = self.engine_api.canonical_stream.next().await.unwrap();
        let tx = head.tip().transactions().next().unwrap();
        assert_eq!(tx.hash().as_slice(), tip_tx_hash.as_slice());

        // wait for the block to commit
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        // make sure the block hash we submitted via FCU engine api is the new latest block
        // using an RPC call
        let latest_block =
            self.inner.provider.block_by_number_or_tag(BlockNumberOrTag::Latest)?.unwrap();
        assert_eq!(latest_block.hash_slow(), block_hash);
        Ok(())
    }
}
