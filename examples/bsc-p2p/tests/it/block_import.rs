use alloy_primitives::BlockNumber;
use example_bsc_p2p::{
    chainspec::{boot_nodes, bsc_chain_spec, head},
    handshake::BscHandshake,
};
use reth_discv4::Discv4ConfigBuilder;
use reth_eth_wire::GetBlockBodies;
use reth_network::{
    EthNetworkPrimitives, NetworkConfig, NetworkEvent, NetworkEventListenerProvider,
    NetworkManager, PeerRequest,
};
use reth_provider::noop::NoopProvider;
use secp256k1::{rand, SecretKey};
use std::{
    net::{Ipv4Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};
use tokio_stream::StreamExt;
use tracing::info;

#[tokio::test(flavor = "multi_thread")]
async fn can_import_block() {
    reth_tracing::init_test_tracing();
    let local_addr = SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), 30303);

    let secret_key = SecretKey::new(&mut rand::thread_rng());

    let net_cfg = NetworkConfig::<_, EthNetworkPrimitives>::builder(secret_key)
        .boot_nodes(boot_nodes())
        .set_head(head())
        .with_pow()
        .listener_addr(local_addr)
        .eth_rlpx_handshake(Arc::new(BscHandshake::default()))
        .build(NoopProvider::eth(bsc_chain_spec()));

    let net_cfg = net_cfg.set_discovery_v4(
        Discv4ConfigBuilder::default()
            .add_boot_nodes(boot_nodes())
            .lookup_interval(Duration::from_millis(500))
            .build(),
    );
    let net_manager = NetworkManager::<EthNetworkPrimitives>::new(net_cfg).await.unwrap();

    let net_handle = net_manager.handle().clone();
    let mut events = net_handle.event_listener();

    tokio::spawn(net_manager);

    while let Some(evt) = events.next().await {
        if let NetworkEvent::ActivePeerSession { info, messages } = evt {
            info!("Active peer session: {:?}", info);
            // Send block body request
            // it should trigger a block import
            // expect the chain to progress
            // let request = PeerRequest::GetBlockBodies {
            //     request: GetBlockBodies { block_hashes: vec![], max_block_bodies: 1 },
            //     response: oneshot::channel(),
            // };
            messages.try_send(request).unwrap();
        }
    }
}
