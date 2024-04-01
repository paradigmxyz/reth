use crate::test_suite::TestSuite;
use futures_util::StreamExt;
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
    let tasks = TaskManager::current();
    let test_suite = TestSuite::new();

    // Node setup
    let node_config = NodeConfig::test()
        .with_chain(test_suite.chain_spec.clone())
        .with_rpc(RpcServerArgs::default().with_http());

    let NodeHandle { mut node, node_exit_future: _ } = NodeBuilder::new(node_config)
        .testing_node(tasks.executor())
        .node(EthereumNode::default())
        .launch()
        .await?;

    // setup engine api events and payload service events
    let mut notifications = node.provider.canonical_state_stream();
    let payload_events = node.payload_builder.subscribe().await?;

    // push tx into pool via RPC server
    let eth_api = node.rpc_registry.eth_api();
    let transfer_tx = test_suite.transfer_tx();
    eth_api.send_raw_transaction(transfer_tx.envelope_encoded()).await?;

    // trigger new payload building draining the pool
    let eth_attr = eth_payload_attributes();
    let payload_id = node.payload_builder.new_payload(eth_attr.clone()).await?;

    // resolve best payload via engine api
    let client = node.engine_http_client();

    // ensure we can get the payload over the engine api
    let _payload = client.get_payload_v3(payload_id).await?;

    let mut payload_event_stream = payload_events.into_stream();

    // first event is the payload attributes
    let first_event = payload_event_stream.next().await.unwrap()?;
    if let reth::payload::Events::Attributes(attr) = first_event {
        assert_eq!(eth_attr.timestamp, attr.timestamp);
    } else {
        panic!("Expect first event as payload attributes.")
    }

    // second event is built payload
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
        let tx = head.tip().transactions().next().unwrap();
        assert_eq!(tx.hash(), transfer_tx.hash);

        // make sure the block hash we submitted via FCU engine api is the new latest block using an
        // RPC call
        let latest_block = node.provider.block_by_number_or_tag(BlockNumberOrTag::Latest)?.unwrap();
        assert_eq!(latest_block.hash_slow(), hash);
    } else {
        panic!("Expect a built payload event.");
    }

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
