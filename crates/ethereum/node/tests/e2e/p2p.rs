use crate::utils::{advance_with_random_transactions, eth_payload_attributes};
use alloy_consensus::{SignableTransaction, TxEip1559, TxEnvelope};
use alloy_eips::Encodable2718;
use alloy_network::TxSignerSync;
use alloy_provider::{Provider, ProviderBuilder};
use futures::future::JoinAll;
use rand::{rngs::StdRng, seq::IndexedRandom, Rng, SeedableRng};
use reth_chainspec::{ChainSpecBuilder, MAINNET};
use reth_e2e_test_utils::{
    setup, setup_engine, setup_engine_with_connection, transaction::TransactionTestContext,
    wallet::Wallet,
};
use reth_network::{NetworkInfo, PeersInfo};
use reth_node_builder::{NodeBuilder, NodeHandle};
use reth_node_core::{args::NetworkArgs, node_config::NodeConfig};
use reth_node_ethereum::EthereumNode;
use reth_rpc_api::EthApiServer;
use reth_tasks::Runtime;
use std::{net::UdpSocket, sync::Arc, time::Duration};

#[tokio::test]
async fn can_launch_with_net_if_and_shared_discovery_port() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let runtime = Runtime::test();
    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
            .cancun_activated()
            .build(),
    );

    let discovery_port = unused_udp_port();
    let mut network = NetworkArgs::default().with_unused_p2p_port();
    network.bootnodes = Some(Vec::new());
    network.net_if = Some(loopback_net_if().to_string());
    network.discovery.disable_dns_discovery = true;
    network.discovery.disable_nat = true;
    network.discovery.port = discovery_port;
    network.discovery.discv5_port = None;
    network.discovery.discv5_port_ipv6 = None;

    let node_config = NodeConfig::test().with_chain(chain_spec).with_network(network);
    let NodeHandle { node, node_exit_future: _ } = NodeBuilder::new(node_config)
        .testing_node(runtime)
        .node(EthereumNode::default())
        .launch()
        .await?;

    assert!(node.network.discv4().is_some());
    assert!(node.network.discv5().is_some());
    assert!(node.network.local_addr().ip().is_loopback());

    let local_node_record = node.network.local_node_record();
    let discv5_port = node.network.discv5().expect("discv5 should be enabled").local_port();
    assert!(local_node_record.address.is_loopback());
    assert_eq!(local_node_record.udp_port, discovery_port);
    assert_eq!(discv5_port, discovery_port);

    Ok(())
}

#[cfg(any(
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "macos",
    target_os = "netbsd",
    target_os = "openbsd"
))]
const fn loopback_net_if() -> &'static str {
    "lo0"
}

#[cfg(not(any(
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "macos",
    target_os = "netbsd",
    target_os = "openbsd"
)))]
const fn loopback_net_if() -> &'static str {
    "lo"
}

fn unused_udp_port() -> u16 {
    UdpSocket::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .expect("failed to bind temporary UDP socket")
        .local_addr()
        .expect("failed to read temporary UDP socket address")
        .port()
}

#[tokio::test]
async fn can_sync() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let (mut nodes, wallet) = setup::<EthereumNode>(
        2,
        Arc::new(
            ChainSpecBuilder::default()
                .chain(MAINNET.chain)
                .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
                .cancun_activated()
                .build(),
        ),
        false,
        eth_payload_attributes,
    )
    .await?;

    let raw_tx = TransactionTestContext::transfer_tx_bytes(1, wallet.inner).await;
    let mut second_node = nodes.pop().unwrap();
    let mut first_node = nodes.pop().unwrap();

    // Make the first node advance
    let tx_hash = first_node.rpc.inject_tx(raw_tx).await?;

    // make the node advance
    let payload = first_node.advance_block().await?;

    let block_hash = payload.block().hash();
    let block_number = payload.block().number;

    // assert the block has been committed to the blockchain
    first_node.assert_new_block(tx_hash, block_hash, block_number).await?;

    // only send forkchoice update to second node
    second_node.update_forkchoice(block_hash, block_hash).await?;

    // expect second node advanced via p2p gossip
    second_node.assert_new_block(tx_hash, block_hash, 1).await?;

    Ok(())
}

#[tokio::test]
async fn e2e_test_send_transactions() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let seed: [u8; 32] = rand::rng().random();
    let mut rng = StdRng::from_seed(seed);
    println!("Seed: {seed:?}");

    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
            .cancun_activated()
            .prague_activated()
            .build(),
    );

    let (mut nodes, _) = setup_engine::<EthereumNode>(
        2,
        chain_spec.clone(),
        false,
        Default::default(),
        eth_payload_attributes,
    )
    .await?;
    let mut node = nodes.pop().unwrap();
    let provider = ProviderBuilder::new().connect_http(node.rpc_url());

    advance_with_random_transactions(&mut node, 100, &mut rng, true).await?;

    let second_node = nodes.pop().unwrap();
    let second_provider = ProviderBuilder::new().connect_http(second_node.rpc_url());

    assert_eq!(second_provider.get_block_number().await?, 0);

    let head = provider.get_block_by_number(Default::default()).await?.unwrap().header.hash;

    second_node.sync_to(head).await?;

    Ok(())
}

#[tokio::test]
async fn test_long_reorg() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let seed: [u8; 32] = rand::rng().random();
    let mut rng = StdRng::from_seed(seed);
    println!("Seed: {seed:?}");

    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
            .cancun_activated()
            .prague_activated()
            .build(),
    );

    let (mut nodes, _) = setup_engine::<EthereumNode>(
        2,
        chain_spec.clone(),
        false,
        Default::default(),
        eth_payload_attributes,
    )
    .await?;

    let mut first_node = nodes.pop().unwrap();
    let mut second_node = nodes.pop().unwrap();

    let first_provider = ProviderBuilder::new().connect_http(first_node.rpc_url());

    // Advance first node 100 blocks.
    advance_with_random_transactions(&mut first_node, 100, &mut rng, false).await?;

    // Sync second node to 20th block.
    let head = first_provider.get_block_by_number(20.into()).await?.unwrap();
    second_node.sync_to(head.header.hash).await?;

    // Produce a fork chain with blocks 21..60
    second_node.payload.timestamp = head.header.timestamp;
    advance_with_random_transactions(&mut second_node, 40, &mut rng, true).await?;

    // Reorg first node from 100th block to new 60th block.
    first_node.sync_to(second_node.block_hash(60)).await?;

    // Advance second node 20 blocks and ensure that first node is able to follow it.
    advance_with_random_transactions(&mut second_node, 20, &mut rng, true).await?;
    first_node.sync_to(second_node.block_hash(80)).await?;

    // Ensure that it works the other way around too.
    advance_with_random_transactions(&mut first_node, 20, &mut rng, true).await?;
    second_node.sync_to(first_node.block_hash(100)).await?;

    Ok(())
}

#[tokio::test]
async fn test_reorg_through_backfill() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let seed: [u8; 32] = rand::rng().random();
    let mut rng = StdRng::from_seed(seed);
    println!("Seed: {seed:?}");

    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
            .cancun_activated()
            .prague_activated()
            .build(),
    );

    let (mut nodes, _) = setup_engine::<EthereumNode>(
        2,
        chain_spec.clone(),
        false,
        Default::default(),
        eth_payload_attributes,
    )
    .await?;

    let mut first_node = nodes.pop().unwrap();
    let mut second_node = nodes.pop().unwrap();

    let first_provider = ProviderBuilder::new().connect_http(first_node.rpc_url());

    // Advance first node 100 blocks and finalize the chain.
    advance_with_random_transactions(&mut first_node, 100, &mut rng, true).await?;

    // Sync second node to 20th block.
    let head = first_provider.get_block_by_number(20.into()).await?.unwrap();
    second_node.sync_to(head.header.hash).await?;

    // Produce an unfinalized fork chain with 30 blocks
    second_node.payload.timestamp = head.header.timestamp;
    advance_with_random_transactions(&mut second_node, 30, &mut rng, false).await?;

    // Now reorg second node to the finalized canonical head
    let head = first_provider.get_block_by_number(100.into()).await?.unwrap();
    second_node.sync_to(head.header.hash).await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_tx_propagation() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
            .cancun_activated()
            .prague_activated()
            .build(),
    );

    // Setup wallet
    let chain_id = chain_spec.chain().into();
    let wallet = Wallet::new(1).inner;
    let mut nonce = 0;
    let mut build_tx = || {
        let mut tx = TxEip1559 {
            chain_id,
            max_priority_fee_per_gas: 1_000_000_000,
            max_fee_per_gas: 1_000_000_000,
            gas_limit: 100_000,
            nonce,
            ..Default::default()
        };
        nonce += 1;
        let signature = wallet.sign_transaction_sync(&mut tx).unwrap();
        TxEnvelope::Eip1559(tx.into_signed(signature))
    };

    // Setup 10 nodes
    let (mut nodes, _) = setup_engine_with_connection::<EthereumNode>(
        10,
        chain_spec.clone(),
        false,
        Default::default(),
        eth_payload_attributes,
        false,
    )
    .await?;

    // Connect all nodes to the first one
    let (first, rest) = nodes.split_at_mut(1);
    for node in rest {
        node.connect(&mut first[0]).await;
    }

    // Advance all nodes for 1 block so that they don't consider themselves unsynced
    let tx = build_tx();
    nodes[0].rpc.inject_tx(tx.encoded_2718().into()).await?;
    let payload = nodes[0].advance_block().await?;
    nodes[1..]
        .iter_mut()
        .map(|node| async {
            node.submit_payload(payload.clone()).await.unwrap();
            node.sync_to(payload.block().hash()).await.unwrap();
        })
        .collect::<JoinAll<_>>()
        .await;

    // Build and send transaction to first node
    let tx = build_tx();
    let tx_hash = *tx.tx_hash();
    let _ = nodes[0].rpc.inject_tx(tx.encoded_2718().into()).await?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Assert that all nodes have the transaction
    for (i, node) in nodes.iter().enumerate() {
        assert!(
            node.rpc.inner.eth_api().transaction_by_hash(tx_hash).await?.is_some(),
            "Node {i} should have the transaction"
        );
    }

    // Build and send one more transaction to a random node
    let tx = build_tx();
    let tx_hash = *tx.tx_hash();
    let _ = nodes.choose(&mut rand::rng()).unwrap().rpc.inject_tx(tx.encoded_2718().into()).await?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Assert that all nodes have the transaction
    for node in nodes {
        assert!(node.rpc.inner.eth_api().transaction_by_hash(tx_hash).await?.is_some());
    }

    Ok(())
}
