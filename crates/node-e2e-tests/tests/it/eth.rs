use futures_util::StreamExt;
use node_e2e_tests::test_suite::TestSuite;
use reth::{
    builder::{NodeBuilder, NodeHandle},
    payload::EthPayloadBuilderAttributes,
    providers::{BlockReaderIdExt, CanonStateSubscriptions},
    rpc::{
        api::EngineApiClient,
        eth::EthTransactions,
        types::engine::{ExecutionPayloadEnvelopeV3, ForkchoiceState, PayloadAttributes},
    },
    tasks::TaskManager,
};
use reth_node_core::{args::RpcServerArgs, node_config::NodeConfig};
use reth_node_ethereum::EthereumNode;
use reth_primitives::{Address, BlockNumberOrTag, B256};
use std::time::{SystemTime, UNIX_EPOCH};

#[tokio::test]
async fn can_run_eth_node() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();
    let tasks = TaskManager::current();
    let test_suite = TestSuite::new();

    // Node setup
    let node_config = NodeConfig::test()
        .with_chain(test_suite.chain_spec())
        .with_rpc(RpcServerArgs::default().with_unused_ports().with_http());

    let NodeHandle { mut node, node_exit_future: _ } = NodeBuilder::new(node_config)
        .testing_node(tasks.executor())
        .node(EthereumNode::default())
        .launch()
        .await?;

    // setup engine api events and payload service events
    let mut notifications = node.provider.canonical_state_stream();
    let payload_events = node.payload_builder.subscribe().await?;
    let mut payload_event_stream = payload_events.into_stream();

    // push tx into pool via RPC server
    let eth_api = node.rpc_registry.eth_api();
    let (expected_hash, raw_tx) = test_suite.transfer_tx().await;

    eth_api.send_raw_transaction(raw_tx).await?;

    // trigger new payload building draining the pool
    let eth_attr = eth_payload_attributes();
    let payload_id = node.payload_builder.new_payload(eth_attr.clone()).await?;

    // first event is the payload attributes
    let first_event = payload_event_stream.next().await.unwrap()?;
    if let reth::payload::Events::Attributes(attr) = first_event {
        assert_eq!(eth_attr.timestamp, attr.timestamp);
    } else {
        panic!("Expect first event as payload attributes.")
    }

    // wait until an actual payload is built before we resolve it via engine api
    loop {
        let payload = node.payload_builder.best_payload(payload_id).await.unwrap().unwrap();
        if payload.block().body.is_empty() {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            continue;
        }
        break;
    }

    let client = node.engine_http_client();
    // trigger resolve payload via engine api
    let _ = client.get_payload_v3(payload_id).await?;

    // ensure we're also receiving the built payload as event
    let second_event = payload_event_stream.next().await.unwrap()?;
    if let reth::payload::Events::BuiltPayload(payload) = second_event {
        // setup payload for submission
        let envelope_v3 = ExecutionPayloadEnvelopeV3::from(payload);
        let payload_v3 = envelope_v3.execution_payload;

        // submit payload to engine api
        let submission = client
            .new_payload_v3(payload_v3, vec![], eth_attr.parent_beacon_block_root.unwrap())
            .await?;
        assert!(submission.is_valid());

        // get latest valid hash from blockchain tree
        let hash = submission.latest_valid_hash.unwrap();

        // trigger forkchoice update via engine api to commit the block to the blockchain
        let fcu = client
            .fork_choice_updated_v2(
                ForkchoiceState {
                    head_block_hash: hash,
                    safe_block_hash: hash,
                    finalized_block_hash: hash,
                },
                None,
            )
            .await?;
        assert!(fcu.is_valid());

        // get head block from notifications stream and verify the tx has been pushed to the pool
        // is actually present in the canonical block
        let head = notifications.next().await.unwrap();
        let tx = head.tip().transactions().next();
        assert_eq!(tx.unwrap().hash().as_slice(), expected_hash.as_slice());

        // make sure the block hash we submitted via FCU engine api is the new latest block using an
        // RPC call
        let latest_block = node.provider.block_by_number_or_tag(BlockNumberOrTag::Latest)?.unwrap();
        assert_eq!(latest_block.hash_slow(), hash);
    } else {
        panic!("Expect a built payload event.");
    }

    Ok(())
}

#[tokio::test]
#[cfg(unix)]
async fn can_run_eth_node_with_auth_engine_api_over_ipc() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();
    let tasks = TaskManager::current();
    let test_suite = TestSuite::new();

    // Node setup
    let node_config = NodeConfig::test()
        .with_chain(test_suite.chain_spec())
        .with_rpc(RpcServerArgs::default().with_unused_ports().with_http().with_auth_ipc());

    let NodeHandle { mut node, node_exit_future: _ } = NodeBuilder::new(node_config)
        .testing_node(tasks.executor())
        .node(EthereumNode::default())
        .launch()
        .await?;

    // setup engine api events and payload service events
    let _notifications = node.provider.canonical_state_stream();
    let payload_events = node.payload_builder.subscribe().await?;
    let mut payload_event_stream = payload_events.into_stream();

    // push tx into pool via RPC server
    let eth_api = node.rpc_registry.eth_api();
    let (_expected_hash, raw_tx) = test_suite.transfer_tx().await;

    eth_api.send_raw_transaction(raw_tx).await?;

    // trigger new payload building draining the pool
    let eth_attr = eth_payload_attributes();
    let _payload_id = node.payload_builder.new_payload(eth_attr.clone()).await?;

    // first event is the payload attributes
    let first_event = payload_event_stream.next().await.unwrap()?;
    if let reth::payload::Events::Attributes(attr) = first_event {
        assert_eq!(eth_attr.timestamp, attr.timestamp);
    } else {
        panic!("Expect first event as payload attributes.")
    }

    Ok(())
}

#[tokio::test]
#[cfg(unix)]
async fn test_failed_run_eth_node_with_no_auth_engine_api_over_ipc_opts() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();
    let tasks = TaskManager::current();
    let test_suite = TestSuite::new();

    // Node setup
    let node_config = NodeConfig::test().with_chain(test_suite.chain_spec());

    let NodeHandle { node, node_exit_future: _ } = NodeBuilder::new(node_config)
        .testing_node(tasks.executor())
        .node(EthereumNode::default())
        .launch()
        .await?;

    let client = node.engine_ipc_client().await;
    assert!(client.is_none(), "ipc auth should be disabled by default");

    Ok(())
}
fn eth_payload_attributes() -> EthPayloadBuilderAttributes {
    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();

    let attributes = PayloadAttributes {
        timestamp,
        prev_randao: B256::ZERO,
        suggested_fee_recipient: Address::ZERO,
        withdrawals: Some(vec![]),
        parent_beacon_block_root: Some(B256::ZERO),
    };
    EthPayloadBuilderAttributes::new(B256::ZERO, attributes)
}
