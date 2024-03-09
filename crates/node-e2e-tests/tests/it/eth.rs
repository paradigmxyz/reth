use std::time::{SystemTime, UNIX_EPOCH};

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
use reth_primitives::{hex, Address, B256};

#[tokio::test]
async fn can_run_eth_node() -> eyre::Result<()> {
    let tasks = TaskManager::current();
    // node setup
    let node_config = NodeConfig::test().with_rpc(RpcServerArgs::default().with_http());
    let NodeHandle { mut node, node_exit_future: _ } = NodeBuilder::new(node_config)
        .testing_node(tasks.executor())
        .node(EthereumNode::default())
        .launch()
        .await?;

    // submit raw tx though rpc (<https://etherscan.io/tx/0x7cae1dcfa6c986160c74de00d067b6c0415bc8dbb53ae1916b287e30f4e7ed35>)
    // TODO: find a more convenient way to create account, add balance and create raw tx
    let raw_tx = hex!("f86c808504e3b292008252089422fb1c1a0f32e4243bdb4ab65d427d6f1503760188016345785d8a00008026a0348272ca3d00e9fe519672cdb59957ef5487d09d8fb9983db16b8937e4cbbf3ba020ce87114962eab4f67a90ae0ff95159557529b3055cba965455cd1d65b7120d");
    let eth_api = node.rpc_registry.eth_api();
    eth_api.send_raw_transaction(raw_tx.into()).await.unwrap();

    // trigger new payload build
    let eth_attr = eth_payload_attributes();
    let payload_id = node.payload_builder.new_payload(eth_attr).await.unwrap();

    // get payload and submit it through engine api
    let client = node.auth_server_handle().http_client();

    let payload = EngineApiClient::<EthEngineTypes>::get_payload_v2(&client, payload_id).await?;
    let exec_payload = convert_payload_field_v2_to_payload(payload.execution_payload);
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
        withdrawals: None,
        parent_beacon_block_root: Some(B256::ZERO),
    };
    EthPayloadBuilderAttributes::new(B256::ZERO, attributes)
}
