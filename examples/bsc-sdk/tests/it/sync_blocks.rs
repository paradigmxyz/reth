//! This example shows how to run a custom dev node programmatically and submit a transaction
//! through rpc.

use example_bsc_sdk::{
    chainspec::bsc::{bsc_mainnet, head},
    node::network::{boot_nodes, handshake::BscHandshake},
};
use reth::{
    args::RpcServerArgs,
    builder::{NodeBuilder, NodeConfig, NodeHandle},
    tasks::TaskManager,
};
use reth_discv4::Discv4ConfigBuilder;
use reth_network::{EthNetworkPrimitives, NetworkConfig};
use reth_node_ethereum::{node::EthereumAddOns, EthereumNode};
use reth_provider::noop::NoopProvider;
use secp256k1::{rand, SecretKey};
use std::{
    net::{Ipv4Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn can_sync_blocks() -> eyre::Result<()> {
    let tasks = TaskManager::current();
    let exec = tasks.executor();
    let local_addr = SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), 30303);

    let secret_key = SecretKey::new(&mut rand::thread_rng());

    let net_cfg = NetworkConfig::<_, EthNetworkPrimitives>::builder(secret_key)
        .boot_nodes(boot_nodes())
        .set_head(head())
        .with_pow()
        .listener_addr(local_addr)
        .eth_rlpx_handshake(Arc::new(BscHandshake::default()))
        .build(NoopProvider::eth(bsc_mainnet()));

    let net_cfg = net_cfg.set_discovery_v4(
        Discv4ConfigBuilder::default()
            .add_boot_nodes(boot_nodes())
            .lookup_interval(Duration::from_millis(500))
            .build(),
    );
    let _network = net_cfg.start_network().await?;

    let node_config = NodeConfig::new(bsc_mainnet())
        .with_unused_ports()
        .with_rpc(RpcServerArgs::default().with_unused_ports().with_http());

    // TODO: override network and Evm Executor with BSC types
    let NodeHandle { node: _, node_exit_future: _ } = NodeBuilder::new(node_config.clone())
        .testing_node(exec.clone())
        .with_types::<EthereumNode>()
        .with_components(EthereumNode::components())
        .with_add_ons(EthereumAddOns::default())
        .launch()
        .await?;

    Ok(())
}
