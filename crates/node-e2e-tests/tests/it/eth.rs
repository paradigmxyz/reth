use crate::test_suite::TestSuite;
use reth::{
    builder::{NodeBuilder, NodeHandle},
    payload::EthPayloadBuilderAttributes,
    rpc::{
        api::EngineApiClient,
        compat::engine::payload::convert_payload_field_v2_to_payload,
        eth::EthTransactions,
        types::engine::{ExecutionPayloadInputV2, PayloadAttributes},
    },
    tasks::TaskManager,
};
use reth_node_core::{args::RpcServerArgs, node_config::NodeConfig};
use reth_node_ethereum::{EthEngineTypes, EthereumNode};
use reth_primitives::{Address, B256};
use std::time::{SystemTime, UNIX_EPOCH};

#[tokio::test]
async fn can_run_eth_node() -> eyre::Result<()> {
    let tasks = TaskManager::current();
    let test_suite = TestSuite::new();
    let raw_tx = test_suite.raw_transfer_tx();

    // node setup
    let node_config = NodeConfig::test()
        .with_chain(test_suite.chain_spec)
        .with_rpc(RpcServerArgs::default().with_http());
    let NodeHandle { mut node, node_exit_future: _ } = NodeBuilder::new(node_config)
        .testing_node(tasks.executor())
        .node(EthereumNode::default())
        .launch()
        .await?;

    // send raw transfer via rpc
    let eth_api = node.rpc_registry.eth_api();
    eth_api.send_raw_transaction(raw_tx).await.unwrap();

    // trigger new payload build
    let eth_attr = eth_payload_attributes();
    let payload_id = node.payload_builder.new_payload(eth_attr).await.unwrap();

    // get payload and submit it through engine api
    let client = node.auth_server_handle().http_client();

    let payload = EngineApiClient::<EthEngineTypes>::get_payload_v2(&client, payload_id).await?;
    let exec_payload = convert_payload_field_v2_to_payload(payload.execution_payload);
    println!("payload: {:?}", exec_payload);
    let payload_input = ExecutionPayloadInputV2 {
        execution_payload: exec_payload.as_v1().clone(),
        withdrawals: Some(vec![]),
    };

    let submission =
        EngineApiClient::<EthEngineTypes>::new_payload_v2(&client, payload_input).await?;
    println!("submission: {:?}", submission);

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
