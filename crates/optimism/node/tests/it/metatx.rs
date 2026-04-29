//! Regression tests for disabled Mantle `MetaTx` entry points.

use alloy_genesis::Genesis;
use alloy_network::{
    eip2718::{Decodable2718, Encodable2718},
    Network,
};
use alloy_primitives::{Address, Bytes, TxKind, U256};
use alloy_rpc_types_eth::{TransactionInput, TransactionRequest};
use op_alloy_consensus::OpTxEnvelope;
use op_alloy_network::Optimism;
use reth_chainspec::EthChainSpec;
use reth_db::test_utils::create_test_rw_db_with_path;
use reth_e2e_test_utils::{
    node::NodeTestContext, transaction::TransactionTestContext, wallet::Wallet,
};
use reth_mantle_forks::MANTLE_META_TX_PREFIX;
use reth_node_builder::{EngineNodeLauncher, Node, NodeBuilder, NodeConfig};
use reth_node_core::args::DatadirArgs;
use reth_optimism_chainspec::OpChainSpecBuilder;
use reth_optimism_node::{utils::optimism_payload_attributes, OpNode};
use reth_primitives_traits::WithEncoded;
use reth_provider::providers::BlockchainProvider;
use reth_rpc_eth_api::EthApiServer;
use reth_tasks::TaskManager;

fn mantle_meta_tx_input() -> Bytes {
    let mut input = MANTLE_META_TX_PREFIX.to_vec();
    input.push(0xF8);
    input.into()
}

fn op_rpc_request(
    chain_id: u64,
    from: Address,
    input: Bytes,
) -> <Optimism as Network>::TransactionRequest {
    TransactionRequest {
        chain_id: Some(chain_id),
        from: Some(from),
        to: Some(TxKind::Call(Address::ZERO)),
        gas: Some(100_000),
        max_fee_per_gas: Some(20e9 as u128),
        max_priority_fee_per_gas: Some(20e9 as u128),
        value: Some(U256::ZERO),
        input: TransactionInput::from(input),
        ..Default::default()
    }
    .into()
}

async fn signed_raw_tx(chain_id: u64, wallet: Wallet, input: Bytes) -> Bytes {
    let request = TransactionRequest {
        chain_id: Some(chain_id),
        nonce: Some(0),
        to: Some(TxKind::Call(Address::ZERO)),
        gas: Some(100_000),
        max_fee_per_gas: Some(20e9 as u128),
        max_priority_fee_per_gas: Some(20e9 as u128),
        value: Some(U256::ZERO),
        input: TransactionInput::from(input),
        ..Default::default()
    };

    TransactionTestContext::sign_tx(wallet.inner, request).await.encoded_2718().into()
}

#[tokio::test]
async fn mantle_meta_tx_is_rejected_by_node_rpc_and_payload_builder() {
    reth_tracing::init_test_tracing();

    let genesis: Genesis = serde_json::from_str(include_str!("../assets/genesis.json")).unwrap();
    let chain_spec =
        std::sync::Arc::new(OpChainSpecBuilder::base_mainnet().genesis(genesis).build());
    let chain_id = chain_spec.chain().id();
    let wallet = Wallet::default().with_chain_id(chain_id);
    let from = wallet.inner.address();
    let meta_tx_input = mantle_meta_tx_input();
    let raw_meta_tx = signed_raw_tx(chain_id, wallet, meta_tx_input.clone()).await;
    let op_meta_tx = OpTxEnvelope::decode_2718_exact(raw_meta_tx.as_ref()).unwrap();
    let sequencer_tx = WithEncoded::new(raw_meta_tx.clone(), op_meta_tx);

    let config = NodeConfig::new(chain_spec).with_datadir_args(DatadirArgs {
        datadir: reth_db::test_utils::tempdir_path().into(),
        ..Default::default()
    });
    let db = create_test_rw_db_with_path(
        config
            .datadir
            .datadir
            .unwrap_or_chain_default(config.chain.chain(), config.datadir.clone())
            .db(),
    );
    let tasks = TaskManager::current();
    let node_handle = NodeBuilder::new(config)
        .with_database(db)
        .with_types_and_provider::<OpNode, BlockchainProvider<_>>()
        .with_components(OpNode::new(Default::default()).components())
        .with_add_ons(OpNode::new(Default::default()).add_ons())
        .launch_with_fn(|builder| {
            let launcher = EngineNodeLauncher::new(
                tasks.executor(),
                builder.config.datadir(),
                Default::default(),
            );
            builder.launch_with(launcher)
        })
        .await
        .expect("failed to launch node");

    let mut node = NodeTestContext::new(node_handle.node, move |timestamp| {
        let mut attrs = optimism_payload_attributes(timestamp);
        attrs.transactions = vec![sequencer_tx.clone()];
        attrs.no_tx_pool = true;
        attrs
    })
    .await
    .unwrap();

    let estimate_err = node
        .rpc
        .inner
        .eth_api()
        .estimate_gas(op_rpc_request(chain_id, from, meta_tx_input), None, None)
        .await
        .unwrap_err();
    assert_eq!(estimate_err.code(), -32000);
    assert_eq!(estimate_err.message(), "meta tx is disabled");

    let send_raw_err = node.rpc.inject_tx(raw_meta_tx).await.unwrap_err();
    assert!(
        send_raw_err.to_string().contains("meta tx is disabled"),
        "unexpected sendRawTransaction error: {send_raw_err}",
    );

    let attrs = node.payload.new_payload().await.unwrap();
    node.payload.expect_attr_event(attrs.clone()).await.unwrap();
    let builder_err = node
        .inner
        .payload_builder_handle
        .best_payload(attrs.payload_attributes.id)
        .await
        .expect("payload job should exist")
        .unwrap_err();
    assert_eq!(builder_err.to_string(), "meta tx is disabled");
}
