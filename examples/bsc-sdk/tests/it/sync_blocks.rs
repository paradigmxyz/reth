use alloy_eips::BlockHashOrNumber;
use alloy_primitives::{Address, B256};
use alloy_rpc_types_engine::PayloadAttributes;
use example_bsc_sdk::{
    chainspec::{bsc::bsc_mainnet, BscChainSpec},
    node::BscNode,
};
use reth::{
    args::RpcServerArgs,
    builder::{Node, NodeBuilder, NodeConfig, NodeHandle},
    network::PeerRequest,
    payload::EthPayloadBuilderAttributes,
    tasks::TaskManager,
};
use reth_eth_wire::HeadersDirection;
use reth_eth_wire_types::GetBlockBodies;
use reth_network::BlockDownloaderProvider;

use reth_network_p2p::headers::client::{HeadersClient, HeadersRequest};
use reth_provider::providers::BlockchainProvider;
use std::sync::Arc;
use tokio::sync::oneshot;
use tokio_stream::StreamExt;

fn eth_payload_attributes(timestamp: u64) -> EthPayloadBuilderAttributes {
    let attributes = PayloadAttributes {
        timestamp,
        prev_randao: B256::ZERO,
        suggested_fee_recipient: Address::ZERO,
        withdrawals: Some(vec![]),
        parent_beacon_block_root: Some(B256::ZERO),
    };
    EthPayloadBuilderAttributes::new(B256::ZERO, attributes)
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn can_sync_blocks() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();
    let tasks = TaskManager::current();
    let exec = tasks.executor();

    let bsc_chainspec = BscChainSpec { inner: bsc_mainnet() };

    let node_config = NodeConfig::new(Arc::new(bsc_chainspec))
        .with_unused_ports()
        .with_rpc(RpcServerArgs::default().with_unused_ports().with_http());

    let NodeHandle { node, node_exit_future: _ } = NodeBuilder::new(node_config.clone())
        .testing_node(exec.clone())
        .with_types_and_provider::<BscNode, BlockchainProvider<_>>()
        .with_components(BscNode::default().components_builder())
        .with_add_ons(BscNode::default().add_ons())
        .launch()
        .await?;
    let fetch_client = node.network.fetch_client().await?;

    let headers = fetch_client
        .get_headers(HeadersRequest {
            start: BlockHashOrNumber::Number(1),
            limit: 10,
            direction: HeadersDirection::Rising,
        })
        .await?;

    println!("headers: {:?}", headers);

    Ok(())
}
