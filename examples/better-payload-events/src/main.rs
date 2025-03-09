//! Example for how to instantiate a payload builder that emits events when a better payload is built.
//!
//! Run with
//!
//! ```sh
//! cargo run -p example-better-payload-events -- node --chain=dev
//! ```
//! This launches a regular reth node overriding the engine api payload builder with a 
//! [`reth_basic_payload_builder::BetterPayloadEmitter`].

#![warn(unused_crate_dependencies)]
use alloy_eips::{eip1559::ETHEREUM_BLOCK_GAS_LIMIT_30M, eip2718::Decodable2718};
use alloy_rpc_types_engine::PayloadAttributes;
use builder::BetterPayloadEmitterBuilder;
use reth::{
    builder::components::BasicPayloadServiceBuilder,
    cli::Cli,
    core::primitives::AlloyBlockHeader,
    revm::primitives::{alloy_primitives::BlockHash, Address, B256},
    transaction_pool::{EthPooledTransaction, PoolTransaction, TransactionOrigin, TransactionPool},
};
use reth_e2e_test_utils::{transaction as e2e_tx, wallet as e2e_wallet};
use reth_node_ethereum::{node::EthereumAddOns, EthereumNode};
use reth_payload_builder::{EthBuiltPayload, EthPayloadBuilderAttributes};
use reth_primitives::{transaction::SignedTransaction, PooledTransaction};
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::info;
mod builder;

fn main() {
    let (tx, mut rx) = broadcast::channel::<Arc<EthBuiltPayload>>(16);
    let payload_builder = BasicPayloadServiceBuilder::new(BetterPayloadEmitterBuilder::new(tx));

    Cli::parse_args()
        .run(|builder, _| async move {
            let handle = builder
                .with_types::<EthereumNode>()
                .with_components(EthereumNode::components().payload(payload_builder))
                .with_add_ons(EthereumAddOns::default())
                .launch()
                .await?;

            let node = handle.node.clone();
            let executor = node.task_executor.clone();
            let pool = node.pool().clone();
            let payload_handle = node.payload_builder_handle.clone();

            let wallet = e2e_wallet::Wallet::default();

            executor.spawn(async move {
                info!("Starting tx injection");
                for nonce in 0..1000000 {
                    let tx = new_tx(&wallet, nonce).await;
                    let _ = pool.add_transaction(TransactionOrigin::Local, tx).await;
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            });

            executor.spawn(async move {
                let id = payload_handle.send_new_payload(attrs()).await.unwrap();
                info!("New payload id: {:?}", id);
                while let Ok(better_payload) = rx.recv().await {
                    let gas_used = better_payload.block().gas_used();
                    let usage = gas_used * 100 / ETHEREUM_BLOCK_GAS_LIMIT_30M;
                    info!("Got better payload: gas used: {gas_used}, block usage: {usage}%");
                }
            });

            handle.wait_for_node_exit().await
        })
        .unwrap();
}

async fn new_tx(wallet: &e2e_wallet::Wallet, nonce: u64) -> EthPooledTransaction {
    let data =
        e2e_tx::TransactionTestContext::transfer_tx_bytes(1337, wallet.inner.clone(), nonce).await;
    let decoded = PooledTransaction::decode_2718(&mut data.as_ref()).unwrap();
    EthPooledTransaction::from_pooled(decoded.try_into_recovered().unwrap())
}

fn attrs() -> EthPayloadBuilderAttributes {
    let genesis = BlockHash::ZERO;
    EthPayloadBuilderAttributes::new(
        genesis,
        PayloadAttributes {
            timestamp: 1,
            prev_randao: B256::random(),
            suggested_fee_recipient: Address::ZERO,
            withdrawals: Some(vec![]),
            parent_beacon_block_root: Some(genesis),
        },
    )
}
