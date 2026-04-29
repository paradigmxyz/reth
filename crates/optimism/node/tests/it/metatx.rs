//! Regression tests for disabled Mantle `MetaTx` entry points.

use alloy_consensus::{BlockBody, Header, EMPTY_OMMER_ROOT_HASH};
use alloy_genesis::Genesis;
use alloy_network::{
    eip2718::{Decodable2718, Encodable2718},
    Network,
};
use alloy_primitives::{Address, Bytes, TxKind, B256, B64, U256};
use alloy_rpc_types_engine::PayloadStatusEnum;
use alloy_rpc_types_eth::{TransactionInput, TransactionRequest};
use op_alloy_consensus::OpTxEnvelope;
use op_alloy_network::Optimism;
use op_alloy_rpc_types_engine::OpExecutionData;
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
use reth_optimism_primitives::OpTransactionSigned;
use reth_primitives_traits::{proofs, WithEncoded};
use reth_provider::{providers::BlockchainProvider, HeaderProvider};
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

/// Verifier-mode regression: a block received via engine API `newPayload` that contains
/// a `MetaTx` transaction must be rejected with `PayloadStatus::Invalid`.
///
/// This covers the path where an external sequencer produces a block with a `MetaTx` and
/// a verifier imports it via the Engine API. The rejection fires in
/// `tx_iterator_for_payload` → `ensure_payload_transaction_input_supported`, before
/// signer recovery.
#[tokio::test]
async fn mantle_meta_tx_is_rejected_by_engine_new_payload() {
    reth_tracing::init_test_tracing();

    // ── Node setup (reuse genesis from existing test) ────────────────────
    let genesis: Genesis = serde_json::from_str(include_str!("../assets/genesis.json")).unwrap();
    let chain_spec =
        std::sync::Arc::new(OpChainSpecBuilder::base_mainnet().genesis(genesis).build());
    let chain_id = chain_spec.chain().id();
    let wallet = Wallet::default().with_chain_id(chain_id);

    // Build MetaTx raw bytes
    let meta_tx_input = mantle_meta_tx_input();
    let raw_meta_tx = signed_raw_tx(chain_id, wallet, meta_tx_input).await;

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

    // Use default payload attributes (no injected sequencer txs).
    let node = NodeTestContext::new(node_handle.node, optimism_payload_attributes).await.unwrap();

    // Read genesis header directly from the provider as parent.
    let genesis_header = node
        .inner
        .provider
        .header_by_number(0)
        .expect("provider should not error")
        .expect("genesis header must exist");
    let parent_hash = genesis_header.hash_slow();
    let parent_number = genesis_header.number;
    let parent_timestamp = genesis_header.timestamp;
    let parent_base_fee = genesis_header.base_fee_per_gas.unwrap_or(1_000_000_000);

    // ── Build a synthetic block containing the MetaTx ────────────────────
    let meta_tx = OpTransactionSigned::decode_2718(&mut raw_meta_tx.as_ref())
        .expect("valid signed EIP-1559 tx");
    let transactions_root = proofs::calculate_transaction_root(&[&meta_tx]);

    // Genesis timestamp=0 is pre-Canyon (pre-Shanghai), so no withdrawals fields.
    let header = Header {
        parent_hash,
        ommers_hash: EMPTY_OMMER_ROOT_HASH,
        beneficiary: Address::ZERO,
        state_root: B256::ZERO, // intentionally wrong; MetaTx fires before state validation
        transactions_root,
        receipts_root: B256::ZERO, // intentionally wrong; same reason
        logs_bloom: Default::default(),
        difficulty: U256::ZERO,
        number: parent_number + 1,
        gas_limit: 30_000_000,
        gas_used: 0,
        timestamp: parent_timestamp + 2,
        mix_hash: B256::ZERO,
        nonce: B64::ZERO,
        base_fee_per_gas: Some(parent_base_fee),
        withdrawals_root: None,
        blob_gas_used: None,
        excess_blob_gas: None,
        parent_beacon_block_root: None,
        requests_hash: None,
        extra_data: Default::default(),
    };

    let block = alloy_consensus::Block {
        header,
        body: BlockBody { transactions: vec![meta_tx], ommers: vec![], withdrawals: None },
    };

    // from_block_slow computes the correct block hash from the header.
    let execution_data = OpExecutionData::from_block_slow(&block);

    // ── Submit via engine API newPayload ──────────────────────────────────
    let result = node.inner.add_ons_handle.beacon_engine_handle.new_payload(execution_data).await;

    // ── Assert MetaTx rejection ──────────────────────────────────────────
    // The engine may return the MetaTx error as:
    //   - Err(Internal(BlockExecutionError(Other(MetaTxDisabled))))
    //   - Ok(PayloadStatus::Invalid { validation_error })
    // Either way, the error message must contain "meta tx is disabled".
    let error_msg = match result {
        Err(err) => err.to_string(),
        Ok(status) => match status.status {
            PayloadStatusEnum::Invalid { validation_error } => validation_error,
            other => panic!("expected Invalid payload status or error, got: {other:?}"),
        },
    };
    assert!(
        error_msg.contains("meta tx is disabled"),
        "expected 'meta tx is disabled', got: {error_msg}",
    );
}
