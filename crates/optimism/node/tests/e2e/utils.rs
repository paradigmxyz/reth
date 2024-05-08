use reth::{rpc::types::engine::PayloadAttributes, tasks::TaskManager};
use reth_e2e_test_utils::{transaction::TransactionTestContext, wallet::Wallet, NodeHelperType};
use reth_node_optimism::{OptimismBuiltPayload, OptimismNode, OptimismPayloadBuilderAttributes};
use reth_payload_builder::EthPayloadBuilderAttributes;
use reth_primitives::{Address, ChainSpecBuilder, Genesis, B256, BASE_MAINNET};
use std::sync::Arc;
use tokio::sync::Mutex;

/// Optimism Node Helper type
pub(crate) type OpNode = NodeHelperType<OptimismNode>;

pub(crate) async fn setup(num_nodes: usize) -> eyre::Result<(Vec<OpNode>, TaskManager, Wallet)> {
    let genesis: Genesis = serde_json::from_str(include_str!("../assets/genesis.json")).unwrap();
    reth_e2e_test_utils::setup(
        num_nodes,
        Arc::new(
            ChainSpecBuilder::default()
                .chain(BASE_MAINNET.chain)
                .genesis(genesis)
                .ecotone_activated()
                .build(),
        ),
        false,
    )
    .await
}

/// Advance the chain with sequential payloads returning them in the end.
pub(crate) async fn advance_chain(
    length: usize,
    node: &mut OpNode,
    wallet: Arc<Mutex<Wallet>>,
) -> eyre::Result<Vec<(OptimismBuiltPayload, OptimismPayloadBuilderAttributes)>> {
    node.advance(
        length as u64,
        |_| {
            let wallet = wallet.clone();
            Box::pin(async move {
                let mut wallet = wallet.lock().await;
                let tx_fut = TransactionTestContext::optimism_l1_block_info_tx(
                    wallet.chain_id,
                    wallet.inner.clone(),
                    wallet.inner_nonce,
                );
                wallet.inner_nonce += 1;
                tx_fut.await
            })
        },
        optimism_payload_attributes,
    )
    .await
}

/// Helper function to create a new eth payload attributes
pub(crate) fn optimism_payload_attributes(timestamp: u64) -> OptimismPayloadBuilderAttributes {
    let attributes = PayloadAttributes {
        timestamp,
        prev_randao: B256::ZERO,
        suggested_fee_recipient: Address::ZERO,
        withdrawals: Some(vec![]),
        parent_beacon_block_root: Some(B256::ZERO),
    };

    OptimismPayloadBuilderAttributes {
        payload_attributes: EthPayloadBuilderAttributes::new(B256::ZERO, attributes),
        transactions: vec![],
        no_tx_pool: false,
        gas_limit: Some(30_000_000),
    }
}
