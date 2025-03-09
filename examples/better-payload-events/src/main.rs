//! Example for how to instantiate a payload builder that emits events when a better payload is built.
//!
//! Run with
//!
//! ```sh
//! cargo run -p example-better-payload-events -- node --chain=dev
//! ```
//!
//! This launches a regular reth node overriding the engine api payload builder with a [`reth_basic_payload_builder::BetterPayloadEmitter`].

#![warn(unused_crate_dependencies)]
use alloy_eips::eip2718::Decodable2718;
use builder::BetterPayloadEmitterBuilder;
use reth::{
    builder::components::BasicPayloadServiceBuilder,
    cli::Cli,
    transaction_pool::{EthPooledTransaction, PoolTransaction, TransactionOrigin, TransactionPool},
};
use reth_e2e_test_utils::{transaction as e2e_tx, wallet as e2e_wallet};
use reth_node_ethereum::{node::EthereumAddOns, EthereumNode};
use reth_payload_builder::EthBuiltPayload;
use reth_primitives::{transaction::SignedTransaction, PooledTransaction};
use tokio::sync::broadcast;
use tracing::info;
mod builder;

fn main() {
    let (tx, rx) = broadcast::channel::<EthBuiltPayload>(16);
    let payload_builder = BasicPayloadServiceBuilder::new(BetterPayloadEmitterBuilder::new(tx));

    Cli::parse_args()
        .run(|builder, _| async move {
            let handle = builder
                .with_types::<EthereumNode>()
                .with_components(EthereumNode::components().payload(payload_builder))
                .with_add_ons(EthereumAddOns::default())
                .launch()
                .await?;

            let executor = handle.node.task_executor.clone();
            let pool = handle.node.pool().clone();
            let wallet = e2e_wallet::Wallet::default();

            executor.spawn(async move {
                info!("Starting tx injection");
                for nonce in 0..100000 {
                    let tx = new_tx(&wallet, nonce).await;
                    pool.add_transaction(TransactionOrigin::Local, tx).await.expect("inject");
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
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
