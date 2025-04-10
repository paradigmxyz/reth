//! Example of how to use the network with a proxy for eth requests handling.
//!
//! This connects two peers:
//!  - first peer installs a channel for incoming eth request
//!  - second peer connects to first peer and sends a header request
//!
//! Run with
//!
//! ```sh
//! cargo run --release -p example-network-proxy
//! ```

#![warn(unused_crate_dependencies)]

use futures::StreamExt;
use reth_ethereum::{
    chainspec::DEV,
    network::{
        config::rng_secret_key,
        eth_requests::IncomingEthRequest,
        p2p::HeadersClient,
        transactions::NetworkTransactionEvent,
        types::{BlockHashOrNumber, NewPooledTransactionHashes68},
        BlockDownloaderProvider, FetchClient, NetworkConfig, NetworkEventListenerProvider,
        NetworkHandle, NetworkInfo, NetworkManager, Peers,
    },
};

#[tokio::main]
async fn main() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    // The key that's used for encrypting sessions and to identify our node.
    let local_key = rng_secret_key();

    // Configure the network
    let config = NetworkConfig::builder(local_key).build_with_noop_provider(DEV.clone());

    let (requests_tx, mut requests_rx) = tokio::sync::mpsc::channel(1000);
    let (transactions_tx, mut transactions_rx) = tokio::sync::mpsc::unbounded_channel();

    // create the network instance
    let network = NetworkManager::eth(config)
        .await?
        // install the channel through which the network sends incoming eth requests
        .with_eth_request_handler(requests_tx)
        // install the channel through which the network sends incoming transaction messages
        .with_transactions(transactions_tx);

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

    loop {
        // receive incoming eth requests and transaction messages from the second peer
        tokio::select! {
              eth_request = requests_rx.recv() => {
                    let Some(eth_request) = eth_request else {break};
                    match eth_request {
                        IncomingEthRequest::GetBlockHeaders { peer_id, request, response } => {
                            println!("Received block headers request: {peer_id:?}, {request:?}");
                            response.send(Ok(vec![DEV.genesis_header().clone()].into())).unwrap();
                        }
                        IncomingEthRequest::GetBlockBodies { .. } => {}
                        IncomingEthRequest::GetNodeData { .. } => {}
                        IncomingEthRequest::GetReceipts { .. } => {}
                    }
             }
             transaction_message = transactions_rx.recv() => {
                let Some(transaction_message) = transaction_message else {break};
                match transaction_message {
                    NetworkTransactionEvent::IncomingTransactions { .. } => {}
                    NetworkTransactionEvent::IncomingPooledTransactionHashes { peer_id, msg } => {
                        println!("Received incoming tx hashes broadcast: {peer_id:?}, {msg:?}");
                    }
                    NetworkTransactionEvent::GetPooledTransactions { .. } => {}
                    NetworkTransactionEvent::GetTransactionsHandle(_) => {}
                }
            }
        }
    }

    Ok(())
}

/// Launches another network/peer, connects to the first peer and sends requests/messages to the
/// first peer.
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
    // this will establish a connection to the first peer
    peer.add_trusted_peer(*handle.peer_id(), handle.local_addr());

    // obtain the client that can emit requests
    let client: FetchClient = peer.fetch_client().await?;

    let header = client.get_header(BlockHashOrNumber::Number(0)).await.unwrap();
    println!("Got header: {:?}", header);

    // send a (bogus) hashes message
    let hashes = NewPooledTransactionHashes68 {
        types: vec![1],
        sizes: vec![2],
        hashes: vec![Default::default()],
    };
    peer.send_transactions_hashes(*handle.peer_id(), hashes.into());

    Ok(())
}
