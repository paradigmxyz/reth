use crate::test_suite::TestSuite;
use futures_util::StreamExt;
use reth::{
    builder::{NodeBuilder, NodeHandle},
    payload::EthPayloadBuilderAttributes,
    providers::{BlockReaderIdExt, CanonStateSubscriptions},
    rpc::{
        api::EngineApiClient,
        compat::engine::payload::convert_payload_field_v2_to_payload,
        eth::EthTransactions,
        types::engine::{ExecutionPayloadInputV2, ForkchoiceState, PayloadAttributes},
    },
    tasks::TaskManager,
};
use reth_node_core::{args::RpcServerArgs, node_config::NodeConfig};
use reth_node_ethereum::{EthEngineTypes, EthereumNode};
use reth_primitives::{Address, BlockNumberOrTag, B256};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[tokio::test]
async fn can_run_eth_node() -> eyre::Result<()> {
    let tasks = TaskManager::current();
    let test_suite = TestSuite::new();
    let transfer_tx = test_suite.transfer_tx();

    let node_config = NodeConfig::test()
        .with_chain(test_suite.chain_spec)
        .with_rpc(RpcServerArgs::default().with_http());

    let NodeHandle { mut node, node_exit_future: _ } = NodeBuilder::new(node_config)
        .testing_node(tasks.executor())
        .node(EthereumNode::default())
        .launch()
        .await?;

    let mut notifications = node.provider.canonical_state_stream();

    let eth_api = node.rpc_registry.eth_api();
    eth_api.send_raw_transaction(transfer_tx.envelope_encoded()).await?;

    let eth_attr = eth_payload_attributes();
    let payload_id = node.payload_builder.new_payload(eth_attr).await?;
    // give it some time to build
    tokio::time::sleep(Duration::from_millis(10)).await;

    let client = node.auth_server_handle().http_client();

    let payload = EngineApiClient::<EthEngineTypes>::get_payload_v2(&client, payload_id).await?;
    let exec_payload = convert_payload_field_v2_to_payload(payload.execution_payload);
    let payload_input = ExecutionPayloadInputV2 {
        execution_payload: exec_payload.as_v1().clone(),
        withdrawals: Some(vec![]),
    };

    let submission =
        EngineApiClient::<EthEngineTypes>::new_payload_v2(&client, payload_input).await?;
    assert!(submission.is_valid());

    let hash = submission.latest_valid_hash.unwrap();
    let fcu = EngineApiClient::<EthEngineTypes>::fork_choice_updated_v2(
        &client,
        ForkchoiceState {
            head_block_hash: hash,
            safe_block_hash: hash,
            finalized_block_hash: hash,
        },
        None,
    )
    .await?;
    assert!(fcu.is_valid());

    let head = notifications.next().await.unwrap();
    let tx = head.tip().transactions().next().unwrap();
    assert_eq!(tx.hash(), transfer_tx.hash);

    let latest_block = node.provider.block_by_number_or_tag(BlockNumberOrTag::Latest)?.unwrap();
    assert_eq!(latest_block.hash_slow(), hash);

    Ok(())
}

fn eth_payload_attributes() -> EthPayloadBuilderAttributes {
    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();

    let attributes = PayloadAttributes {
        timestamp,
        prev_randao: B256::ZERO,
        suggested_fee_recipient: Address::ZERO,
        withdrawals: Some(vec![]),
        parent_beacon_block_root: None,
    };
    EthPayloadBuilderAttributes::new(B256::ZERO, attributes)
}
