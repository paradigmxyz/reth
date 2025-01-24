//! Example of how to use the network with a proxy for eth requests handling.
//!
//! This connects two peers:
//!  - first peer installs a channel for incoming eth request
//!  - second peer connects to first peer and sends a header request
//!
//! Run with
//!
//! ```not_rust
//! cargo run --release -p example-network-proxy
//! ```

use futures::StreamExt;
use reth_chainspec::DEV;
use reth_network::{
    config::rng_secret_key, eth_requests::IncomingEthRequest, p2p::HeadersClient,
    BlockDownloaderProvider, FetchClient, NetworkConfig, NetworkEventListenerProvider,
    NetworkHandle, NetworkInfo, NetworkManager, Peers,
};

#[tokio::main]
async fn main() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    // The key that's used for encrypting sessions and to identify our node.
    let local_key = rng_secret_key();

    // Configure the network
    let config = NetworkConfig::builder(local_key).build_with_noop_provider(DEV.clone());

    let (tx, mut rx) = tokio::sync::mpsc::channel(1000);

    // create the network instance
    let network = NetworkManager::eth(config)
        .await?
        // install the channel through which the network sends incoming eth requests
        .with_eth_request_handler(tx);

    // get a handle to the network to interact with it
    let handle = network.handle().clone();

    tokio::task::spawn(async move {
        // print network events
        let mut events = handle.event_listener();
        while let Some(event) = events.next().await {
            println!("Received event: {:?}", event);
        }
    });

    let handle = network.handle().clone();

    // spawn the network
    tokio::task::spawn(network);

    // spawn task to fetch a header using another peer/network
    tokio::task::spawn(async move {
        run_peer(handle).await.unwrap();
    });

    while let Some(event) = rx.recv().await {
        match event {
            IncomingEthRequest::GetBlockHeaders { peer_id, request, response } => {
                println!("Received block headers request: {:?}, {:?}", peer_id, request);
                response.send(Ok(vec![DEV.genesis_header().clone()].into())).unwrap();
            }
            IncomingEthRequest::GetBlockBodies { .. } => {}
            IncomingEthRequest::GetNodeData { .. } => {}
            IncomingEthRequest::GetReceipts { .. } => {}
        }
    }

    Ok(())
}

/// Launches another network/peer, connects to the first peer and sends a header request.
async fn run_peer(handle: NetworkHandle) -> eyre::Result<()> {
    // create another peer
    let config = NetworkConfig::builder(rng_secret_key())
        // use random ports
        .with_unused_ports()
        .build_with_noop_provider(DEV.clone());
    let network = NetworkManager::eth(config).await?;
    let peer = network.handle().clone();
    // spawn additional peer
    tokio::task::spawn(network);

    // add the other peer as trusted
    peer.add_trusted_peer(*handle.peer_id(), handle.local_addr());

    // obtain the client that can emit requests
    let client: FetchClient = peer.fetch_client().await?;

    let header = client.get_header(0.into()).await.unwrap();
    println!("Got header: {:?}", header);

    Ok(())
}
