use crate::{ScrollBuiltPayload, ScrollNode as OtherScrollNode, ScrollPayloadBuilderAttributes};
use alloy_genesis::Genesis;
use alloy_primitives::{Address, B256};
use alloy_rpc_types_engine::PayloadAttributes;
use reth_e2e_test_utils::{
    transaction::TransactionTestContext, wallet::Wallet, NodeHelperType, TmpDB,
};
use reth_node_api::NodeTypesWithDBAdapter;

use reth_payload_builder::EthPayloadBuilderAttributes;
use reth_provider::providers::BlockchainProvider;
use reth_scroll_chainspec::ScrollChainSpecBuilder;
use reth_tasks::TaskManager;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Scroll Node Helper type
pub(crate) type ScrollNode = NodeHelperType<
    OtherScrollNode,
    BlockchainProvider<NodeTypesWithDBAdapter<OtherScrollNode, TmpDB>>,
>;

/// Creates the initial setup with `num_nodes` of the node config, started and connected.
pub async fn setup(
    num_nodes: usize,
    is_dev: bool,
) -> eyre::Result<(Vec<ScrollNode>, TaskManager, Wallet)> {
    let genesis: Genesis =
        serde_json::from_str(include_str!("../tests/assets/genesis.json")).unwrap();
    reth_e2e_test_utils::setup_engine(
        num_nodes,
        Arc::new(
            ScrollChainSpecBuilder::scroll_mainnet()
                .genesis(genesis)
                .euclid_v2_activated()
                .build(Default::default()),
        ),
        is_dev,
        Default::default(),
        scroll_payload_attributes,
    )
    .await
}

/// Advance the chain with sequential payloads returning them in the end.
pub async fn advance_chain(
    length: usize,
    node: &mut ScrollNode,
    wallet: Arc<Mutex<Wallet>>,
) -> eyre::Result<Vec<ScrollBuiltPayload>> {
    node.advance(length as u64, |_| {
        let wallet = wallet.clone();
        Box::pin(async move {
            let mut wallet = wallet.lock().await;
            let tx_fut = TransactionTestContext::transfer_tx_nonce_bytes(
                wallet.chain_id,
                wallet.inner.clone(),
                wallet.inner_nonce,
            );
            wallet.inner_nonce += 1;
            tx_fut.await
        })
    })
    .await
}

/// Helper function to create a new scroll payload attributes
pub fn scroll_payload_attributes(timestamp: u64) -> ScrollPayloadBuilderAttributes {
    let attributes = PayloadAttributes {
        timestamp,
        prev_randao: B256::ZERO,
        suggested_fee_recipient: Address::ZERO,
        withdrawals: None,
        parent_beacon_block_root: Some(B256::ZERO),
    };

    ScrollPayloadBuilderAttributes {
        payload_attributes: EthPayloadBuilderAttributes::new(B256::ZERO, attributes),
        transactions: vec![],
        no_tx_pool: false,
        block_data_hint: None,
        gas_limit: None,
    }
}
