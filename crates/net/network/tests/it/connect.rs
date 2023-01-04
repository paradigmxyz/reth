//! Connection tests

use super::testnet::Testnet;
use crate::{NetworkEventStream, PeerConfig};
use enr::{k256::ecdsa::SigningKey, Enr, EnrPublicKey};
use ethers_core::{
    types::{
        transaction::eip2718::TypedTransaction, Address, Bytes, Eip1559TransactionRequest, U64,
    },
    utils::{ChainConfig, CliqueConfig, Genesis, GenesisAccount, Geth},
};
use ethers_middleware::SignerMiddleware;
use ethers_providers::{Http, Middleware, Provider};
use ethers_signers::{LocalWallet, Signer};
use futures::StreamExt;
use reth_discv4::{bootnodes::mainnet_nodes, Discv4Config};
use reth_eth_wire::DisconnectReason;
use reth_interfaces::{
    p2p::headers::client::{HeadersClient, HeadersRequest},
    sync::{SyncState, SyncStateUpdater},
};
use reth_net_common::ban_list::BanList;
use reth_network::{NetworkConfig, NetworkEvent, NetworkManager, PeersConfig};
use reth_primitives::{HeadersDirection, NodeRecord, PeerId};
use reth_provider::test_utils::NoopProvider;
use reth_transaction_pool::test_utils::testing_pool;
use secp256k1::SecretKey;
use std::{
    collections::{HashMap, HashSet},
    io::{BufRead, BufReader},
    net::SocketAddr,
    sync::Arc,
    time::Duration,
};
use tokio::task;

// The timeout for tests that create a GethInstance
const GETH_TIMEOUT: Duration = Duration::from_secs(60);

/// Obtains a PeerId from an ENR. In this case, the PeerId represents the public key contained in
/// the ENR.
fn enr_to_peer_id(enr: Enr<SigningKey>) -> PeerId {
    // In the following tests, methods which accept a public key expect it to contain the public
    // key in its 64-byte encoded (uncompressed) form.
    enr.public_key().encode_uncompressed().into()
}

/// Creates a new Geth with an unused p2p port and temporary data dir.
///
/// Returns the new Geth and the temporary directory.
fn create_new_geth() -> (Geth, tempfile::TempDir) {
    let temp_dir = tempfile::tempdir().expect("should create temp dir");
    let geth = Geth::new().data_dir(temp_dir.path()).p2p_port(unused_port());

    (geth, temp_dir)
}

// copied from ethers-rs
/// A bit of hack to find an unused TCP port.
///
/// Does not guarantee that the given port is unused after the function exists, just that it was
/// unused before the function started (i.e., it does not reserve a port).
fn unused_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .expect("Failed to create TCP listener to find unused port");

    let local_addr =
        listener.local_addr().expect("Failed to read TCP listener local_addr to find unused port");
    local_addr.port()
}

/// Creates two unused SocketAddrs, intended for use as the p2p (TCP) and discovery ports (UDP) for
/// new reth instances.
fn unused_tcp_udp() -> (SocketAddr, SocketAddr) {
    let tcp_listener = std::net::TcpListener::bind("127.0.0.1:0")
        .expect("Failed to create TCP listener to find unused port");
    let tcp_addr = tcp_listener
        .local_addr()
        .expect("Failed to read TCP listener local_addr to find unused port");

    let udp_listener = std::net::UdpSocket::bind("127.0.0.1:0")
        .expect("Failed to create UDP listener to find unused port");
    let udp_addr = udp_listener
        .local_addr()
        .expect("Failed to read UDP listener local_addr to find unused port");

    (tcp_addr, udp_addr)
}

/// Creates a chain config using the given chain id.
/// Funds the given address with max coins.
///
/// Enables all hard forks up to London at genesis.
fn genesis_funded(chain_id: u64, signer_addr: Address) -> Genesis {
    // set up a clique config with a short (1s) period and short (8 block) epoch
    let clique_config = CliqueConfig { period: 1, epoch: 8 };

    let config = ChainConfig {
        chain_id,
        eip155_block: Some(0),
        eip150_block: Some(0),
        eip158_block: Some(0),

        homestead_block: Some(0),
        byzantium_block: Some(0),
        constantinople_block: Some(0),
        petersburg_block: Some(0),
        istanbul_block: Some(0),
        muir_glacier_block: Some(0),
        berlin_block: Some(0),
        london_block: Some(0),
        clique: Some(clique_config),
        ..Default::default()
    };

    // fund account
    let mut alloc = HashMap::new();
    alloc.insert(
        signer_addr,
        GenesisAccount {
            balance: ethers_core::types::U256::MAX,
            nonce: None,
            code: None,
            storage: None,
        },
    );

    // put signer address in the extra data, padded by the required amount of zeros
    // Clique issue: https://github.com/ethereum/EIPs/issues/225
    // Clique EIP: https://eips.ethereum.org/EIPS/eip-225
    //
    // The first 32 bytes are vanity data, so we will populate it with zeros
    // This is followed by the signer address, which is 20 bytes
    // There are 65 bytes of zeros after the signer address, which is usually populated with the
    // proposer signature. Because the genesis does not have a proposer signature, it will be
    // populated with zeros.
    let extra_data_bytes = [&[0u8; 32][..], signer_addr.as_bytes(), &[0u8; 65][..]].concat();
    let extra_data = Bytes::from(extra_data_bytes);

    Genesis {
        config,
        alloc,
        difficulty: ethers_core::types::U256::one(),
        gas_limit: U64::from(5000000),
        extra_data,
        ..Default::default()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_establish_connections() {
    reth_tracing::init_tracing();

    for _ in 0..10 {
        let net = Testnet::create(3).await;

        net.for_each(|peer| assert_eq!(0, peer.num_peers()));

        let mut handles = net.handles();
        let handle0 = handles.next().unwrap();
        let handle1 = handles.next().unwrap();
        let handle2 = handles.next().unwrap();

        drop(handles);
        let handle = net.spawn();

        let listener0 = handle0.event_listener();

        let mut listener1 = handle1.event_listener();
        let mut listener2 = handle2.event_listener();

        handle0.add_peer(*handle1.peer_id(), handle1.local_addr());
        handle0.add_peer(*handle2.peer_id(), handle2.local_addr());

        let mut expected_connections = HashSet::from([*handle1.peer_id(), *handle2.peer_id()]);
        let mut expected_peers = expected_connections.clone();

        // wait for all initiator connections
        let mut established = listener0.take(4);
        while let Some(ev) = established.next().await {
            match ev {
                NetworkEvent::SessionClosed { .. } => {
                    panic!("unexpected event")
                }
                NetworkEvent::SessionEstablished { peer_id, .. } => {
                    assert!(expected_connections.remove(&peer_id))
                }
                NetworkEvent::PeerAdded(peer_id) => {
                    assert!(expected_peers.remove(&peer_id))
                }
                NetworkEvent::PeerRemoved(_) => {
                    panic!("unexpected event")
                }
            }
        }
        assert!(expected_connections.is_empty());
        assert!(expected_peers.is_empty());

        // also await the established session on both target
        futures::future::join(listener1.next(), listener2.next()).await;

        let net = handle.terminate().await;

        assert_eq!(net.peers()[0].num_peers(), 2);
        assert_eq!(net.peers()[1].num_peers(), 1);
        assert_eq!(net.peers()[2].num_peers(), 1);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_already_connected() {
    reth_tracing::init_tracing();
    let mut net = Testnet::default();

    let secret_key = SecretKey::new(&mut rand::thread_rng());
    let client = Arc::new(NoopProvider::default());
    let p1 = PeerConfig::default();

    // initialize two peers with the same identifier
    let p2 = PeerConfig::with_secret_key(Arc::clone(&client), secret_key);
    let p3 = PeerConfig::with_secret_key(Arc::clone(&client), secret_key);

    net.extend_peer_with_config(vec![p1, p2, p3]).await.unwrap();

    let mut handles = net.handles();
    let handle0 = handles.next().unwrap();
    let handle1 = handles.next().unwrap();
    let handle2 = handles.next().unwrap();

    drop(handles);
    let _handle = net.spawn();

    let mut listener0 = NetworkEventStream::new(handle0.event_listener());
    let mut listener2 = NetworkEventStream::new(handle2.event_listener());

    handle0.add_peer(*handle1.peer_id(), handle1.local_addr());

    let peer = listener0.next_session_established().await.unwrap();
    assert_eq!(peer, *handle1.peer_id());

    handle2.add_peer(*handle0.peer_id(), handle0.local_addr());
    let peer = listener2.next_session_established().await.unwrap();
    assert_eq!(peer, *handle0.peer_id());

    let (peer, reason) = listener2.next_session_closed().await.unwrap();
    assert_eq!(peer, *handle0.peer_id());
    let reason = reason.unwrap();
    assert_eq!(reason, DisconnectReason::AlreadyConnected);

    assert_eq!(handle0.num_connected_peers(), 1);
    assert_eq!(handle1.num_connected_peers(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_get_peer() {
    reth_tracing::init_tracing();
    let mut net = Testnet::default();

    let secret_key = SecretKey::new(&mut rand::thread_rng());
    let secret_key_1 = SecretKey::new(&mut rand::thread_rng());
    let client = Arc::new(NoopProvider::default());
    let p1 = PeerConfig::default();
    let p2 = PeerConfig::with_secret_key(Arc::clone(&client), secret_key);
    let p3 = PeerConfig::with_secret_key(Arc::clone(&client), secret_key_1);

    net.extend_peer_with_config(vec![p1, p2, p3]).await.unwrap();

    let mut handles = net.handles();
    let handle0 = handles.next().unwrap();
    let handle1 = handles.next().unwrap();
    let handle2 = handles.next().unwrap();

    drop(handles);
    let _handle = net.spawn();

    let mut listener0 = NetworkEventStream::new(handle0.event_listener());

    handle0.add_peer(*handle1.peer_id(), handle1.local_addr());
    let _ = listener0.next_session_established().await.unwrap();

    handle0.add_peer(*handle2.peer_id(), handle2.local_addr());
    let _ = listener0.next_session_established().await.unwrap();

    let peers = handle0.get_peers().await.unwrap();
    assert_eq!(handle0.num_connected_peers(), peers.len());
    dbg!(peers);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_get_peer_by_id() {
    reth_tracing::init_tracing();
    let mut net = Testnet::default();

    let secret_key = SecretKey::new(&mut rand::thread_rng());
    let secret_key_1 = SecretKey::new(&mut rand::thread_rng());
    let client = Arc::new(NoopProvider::default());
    let p1 = PeerConfig::default();
    let p2 = PeerConfig::with_secret_key(Arc::clone(&client), secret_key);
    let p3 = PeerConfig::with_secret_key(Arc::clone(&client), secret_key_1);

    net.extend_peer_with_config(vec![p1, p2, p3]).await.unwrap();

    let mut handles = net.handles();
    let handle0 = handles.next().unwrap();
    let handle1 = handles.next().unwrap();
    let handle2 = handles.next().unwrap();

    drop(handles);
    let _handle = net.spawn();

    let mut listener0 = NetworkEventStream::new(handle0.event_listener());

    handle0.add_peer(*handle1.peer_id(), handle1.local_addr());
    let _ = listener0.next_session_established().await.unwrap();

    let peer = handle0.get_peer_by_id(*handle1.peer_id()).await.unwrap();
    assert!(peer.is_some());

    let peer = handle0.get_peer_by_id(*handle2.peer_id()).await.unwrap();
    assert!(peer.is_none());
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn test_connect_with_boot_nodes() {
    reth_tracing::init_tracing();
    let secret_key = SecretKey::new(&mut rand::thread_rng());
    let mut discv4 = Discv4Config::builder();
    discv4.add_boot_nodes(mainnet_nodes());

    let config = NetworkConfig::builder(Arc::new(NoopProvider::default()), secret_key)
        .discovery(discv4)
        .build();
    let network = NetworkManager::new(config).await.unwrap();

    let handle = network.handle().clone();
    let mut events = handle.event_listener();
    tokio::task::spawn(network);

    while let Some(ev) = events.next().await {
        dbg!(ev);
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn test_connect_with_builder() {
    reth_tracing::init_tracing();
    let secret_key = SecretKey::new(&mut rand::thread_rng());
    let mut discv4 = Discv4Config::builder();
    discv4.add_boot_nodes(mainnet_nodes());

    let client = Arc::new(NoopProvider::default());
    let config = NetworkConfig::builder(Arc::clone(&client), secret_key).discovery(discv4).build();
    let (handle, network, _, requests) = NetworkManager::new(config)
        .await
        .unwrap()
        .into_builder()
        .request_handler(client)
        .split_with_handle();

    let mut events = handle.event_listener();

    tokio::task::spawn(async move {
        tokio::join!(network, requests);
    });

    let h = handle.clone();
    task::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
            dbg!(h.num_connected_peers());
        }
    });

    while let Some(ev) = events.next().await {
        dbg!(ev);
    }
}

// expects a `ENODE="enode://"` env var that holds the record
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn test_connect_to_trusted_peer() {
    reth_tracing::init_tracing();
    let secret_key = SecretKey::new(&mut rand::thread_rng());
    let discv4 = Discv4Config::builder();

    let client = Arc::new(NoopProvider::default());
    let config = NetworkConfig::builder(Arc::clone(&client), secret_key).discovery(discv4).build();
    let (handle, network, transactions, requests) = NetworkManager::new(config)
        .await
        .unwrap()
        .into_builder()
        .request_handler(client)
        .transactions(testing_pool())
        .split_with_handle();

    let mut events = handle.event_listener();

    tokio::task::spawn(async move {
        tokio::join!(network, requests, transactions);
    });

    let node: NodeRecord = std::env::var("ENODE").unwrap().parse().unwrap();

    handle.add_trusted_peer(node.id, node.tcp_addr());

    let h = handle.clone();
    h.update_sync_state(SyncState::Downloading { target_block: 100 });

    task::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
            dbg!(h.num_connected_peers());
        }
    });

    let fetcher = handle.fetch_client().await.unwrap();

    let headers = fetcher
        .get_headers(HeadersRequest {
            start: 73174u64.into(),
            limit: 10,
            direction: HeadersDirection::Rising,
        })
        .await;

    dbg!(&headers);

    while let Some(ev) = events.next().await {
        dbg!(ev);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_incoming_node_id_blacklist() {
    reth_tracing::init_tracing();
    tokio::time::timeout(GETH_TIMEOUT, async move {
        let secret_key = SecretKey::new(&mut rand::thread_rng());

        // instantiate geth and add ourselves as a peer
        let temp_dir = tempfile::tempdir().unwrap().into_path();
        let geth = Geth::new().data_dir(temp_dir).disable_discovery().spawn();
        let geth_endpoint = SocketAddr::new([127, 0, 0, 1].into(), geth.port());
        let provider = Provider::<Http>::try_from(format!("http://{geth_endpoint}")).unwrap();

        // get the peer id we should be expecting
        let geth_peer_id = enr_to_peer_id(provider.node_info().await.unwrap().enr);

        let ban_list = BanList::new(vec![geth_peer_id], HashSet::new());
        let peer_config = PeersConfig::default().with_ban_list(ban_list);

        let (reth_p2p, reth_disc) = unused_tcp_udp();
        let config = NetworkConfig::builder(Arc::new(NoopProvider::default()), secret_key)
            .listener_addr(reth_p2p)
            .discovery_addr(reth_disc)
            .peer_config(peer_config)
            .build();

        let network = NetworkManager::new(config).await.unwrap();

        let handle = network.handle().clone();
        let events = handle.event_listener();

        tokio::task::spawn(network);

        // make geth connect to us
        let our_enode = NodeRecord::new(reth_p2p, *handle.peer_id());

        provider.add_peer(our_enode.to_string()).await.unwrap();

        let mut event_stream = NetworkEventStream::new(events);

        // check for session to be opened
        let incoming_peer_id = event_stream.next_session_established().await.unwrap();
        assert_eq!(incoming_peer_id, geth_peer_id);

        // check to see that the session was closed
        let incoming_peer_id = event_stream.next_session_closed().await.unwrap().0;
        assert_eq!(incoming_peer_id, geth_peer_id);
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
async fn test_incoming_connect_with_single_geth() {
    reth_tracing::init_tracing();
    tokio::time::timeout(GETH_TIMEOUT, async move {
        let secret_key = SecretKey::new(&mut rand::thread_rng());

        // instantiate geth and add ourselves as a peer
        let temp_dir = tempfile::tempdir().unwrap().into_path();
        let geth = Geth::new().data_dir(temp_dir).disable_discovery().spawn();
        let geth_endpoint = SocketAddr::new([127, 0, 0, 1].into(), geth.port());
        let provider = Provider::<Http>::try_from(format!("http://{geth_endpoint}")).unwrap();

        // get the peer id we should be expecting
        let geth_peer_id = enr_to_peer_id(provider.node_info().await.unwrap().enr);

        let (reth_p2p, reth_disc) = unused_tcp_udp();
        let config = NetworkConfig::builder(Arc::new(NoopProvider::default()), secret_key)
            .listener_addr(reth_p2p)
            .discovery_addr(reth_disc)
            .build();

        let network = NetworkManager::new(config).await.unwrap();

        let handle = network.handle().clone();
        tokio::task::spawn(network);

        let events = handle.event_listener();
        let mut event_stream = NetworkEventStream::new(events);

        // make geth connect to us
        let our_enode = NodeRecord::new(reth_p2p, *handle.peer_id());

        provider.add_peer(our_enode.to_string()).await.unwrap();

        // check for a sessionestablished event
        let incoming_peer_id = event_stream.next_session_established().await.unwrap();
        assert_eq!(incoming_peer_id, geth_peer_id);
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
async fn test_outgoing_connect_with_single_geth() {
    reth_tracing::init_tracing();
    tokio::time::timeout(GETH_TIMEOUT, async move {
        let secret_key = SecretKey::new(&mut rand::thread_rng());

        let (reth_p2p, reth_disc) = unused_tcp_udp();
        let config = NetworkConfig::builder(Arc::new(NoopProvider::default()), secret_key)
            .listener_addr(reth_p2p)
            .discovery_addr(reth_disc)
            .build();
        let network = NetworkManager::new(config).await.unwrap();

        let handle = network.handle().clone();
        tokio::task::spawn(network);

        // create networkeventstream to get the next session established event easily
        let events = handle.event_listener();
        let mut event_stream = NetworkEventStream::new(events);

        // instantiate geth and add ourselves as a peer
        let temp_dir = tempfile::tempdir().unwrap().into_path();
        let geth = Geth::new().disable_discovery().data_dir(temp_dir).spawn();

        let geth_p2p_port = geth.p2p_port().unwrap();
        let geth_socket = SocketAddr::new([127, 0, 0, 1].into(), geth_p2p_port);
        let geth_endpoint = SocketAddr::new([127, 0, 0, 1].into(), geth.port()).to_string();

        let provider = Provider::<Http>::try_from(format!("http://{geth_endpoint}")).unwrap();

        // get the peer id we should be expecting
        let geth_peer_id: PeerId = enr_to_peer_id(provider.node_info().await.unwrap().enr);

        // add geth as a peer then wait for a `SessionEstablished` event
        handle.add_peer(geth_peer_id, geth_socket);

        // check for a sessionestablished event
        let incoming_peer_id = event_stream.next_session_established().await.unwrap();
        assert_eq!(incoming_peer_id, geth_peer_id);
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
async fn test_geth_disconnect() {
    reth_tracing::init_tracing();
    tokio::time::timeout(GETH_TIMEOUT, async move {
        let secret_key = SecretKey::new(&mut rand::thread_rng());

        let (reth_p2p, reth_disc) = unused_tcp_udp();
        let config = NetworkConfig::builder(Arc::new(NoopProvider::default()), secret_key)
            .listener_addr(reth_p2p)
            .discovery_addr(reth_disc)
            .build();
        let network = NetworkManager::new(config).await.unwrap();

        let handle = network.handle().clone();
        tokio::task::spawn(network);

        // create networkeventstream to get the next session established event easily
        let mut events = handle.event_listener();

        // instantiate geth and add ourselves as a peer
        let temp_dir = tempfile::tempdir().unwrap().into_path();
        let geth = Geth::new().disable_discovery().data_dir(temp_dir).spawn();

        let geth_p2p_port = geth.p2p_port().unwrap();
        let geth_socket = SocketAddr::new([127, 0, 0, 1].into(), geth_p2p_port);
        let geth_endpoint = SocketAddr::new([127, 0, 0, 1].into(), geth.port()).to_string();

        let provider = Provider::<Http>::try_from(format!("http://{geth_endpoint}")).unwrap();

        // get the peer id we should be expecting
        let geth_peer_id: PeerId = enr_to_peer_id(provider.node_info().await.unwrap().enr);

        // add geth as a peer then wait for `PeerAdded` and `SessionEstablished` events.
        handle.add_peer(geth_peer_id, geth_socket);

        match events.next().await {
            Some(NetworkEvent::PeerAdded(peer_id)) => assert_eq!(peer_id, geth_peer_id),
            _ => panic!("Expected a peer added event"),
        }

        if let Some(NetworkEvent::SessionEstablished { peer_id, .. }) = events.next().await {
            assert_eq!(peer_id, geth_peer_id);
        } else {
            panic!("Expected a session established event");
        }

        // remove geth as a peer deliberately
        handle.disconnect_peer(geth_peer_id);

        // wait for a disconnect from geth
        if let Some(NetworkEvent::SessionClosed { peer_id, .. }) = events.next().await {
            assert_eq!(peer_id, geth_peer_id);
        } else {
            panic!("Expected a session closed event");
        }
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
async fn sync_from_clique_geth() {
    reth_tracing::init_tracing();
    tokio::time::timeout(GETH_TIMEOUT, async move {
        // first create a signer that we will fund so we can make transactions
        let chain_id = 13337u64;
        let signing_key = SigningKey::random(&mut rand::thread_rng());
        let wallet: LocalWallet = signing_key.clone().into();
        let our_address = wallet.address();

        let (geth, data_dir) = create_new_geth();

        // TODO: remove - this is just so we can see the genesis file created
        // print datadir for debugging and make sure to persist the dir
        let dir_path = data_dir.into_path();
        println!("geth datadir: {dir_path:?}");

        // === fund wallet ===

        // create a pre-funded geth and turn on p2p
        let genesis = genesis_funded(chain_id, wallet.address());
        println!("genesis: {:#?}", genesis);
        let geth = geth.genesis(genesis).chain_id(chain_id).disable_discovery().insecure_unlock();

        // geth starts in dev mode, we can spawn it, mine blocks, and shut it down
        // we need to clone it because we will be reusing the geth config when we restart p2p
        let mut instance = geth.spawn();

        // set up ethers provider
        let geth_endpoint = SocketAddr::new([127, 0, 0, 1].into(), instance.port()).to_string();
        let provider = Provider::<Http>::try_from(format!("http://{geth_endpoint}")).unwrap();
        let provider =
            SignerMiddleware::new_with_provider_chain(provider, wallet.clone()).await.unwrap();

        // === produce blocks ===

        // first get the balance and make sure its not zero
        let balance = provider.get_balance(our_address, None).await.unwrap();
        assert_ne!(balance, 0u64.into());
        println!("address: {our_address:?}");
        println!("balance at genesis: {balance:?}");

        // send the private key to geth and unlock it
        let private_key = signing_key.to_bytes().to_vec().into();
        println!("private key: {}", hex::encode(&private_key));
        let unlocked_addr = provider.import_raw_key(private_key, "".to_string()).await.unwrap();
        assert_eq!(unlocked_addr, our_address);

        let unlock_success =
            provider.unlock_account(our_address, "".to_string(), None).await.unwrap();
        assert!(unlock_success);

        // start mining?
        provider.start_mining(None).await.unwrap();

        // check that we are mining
        let mining = provider.mining().await.unwrap();
        assert!(mining);

        // take the stderr of the geth instance and print it to see more about what geth is doing
        // is it mining blocks? if so can we
        let stderr = instance.stderr().unwrap();

        // print logs in a new task
        task::spawn(async move {
            let mut err_reader = BufReader::new(stderr);

            loop {
                tokio::time::sleep(Duration::from_millis(10)).await;

                let mut buf = String::new();
                if let Ok(line) = err_reader.read_line(&mut buf) {
                    if line == 0 {
                        continue
                    }
                    dbg!(buf);
                }
            }
        });

        // wait for stuff to happen (we are using period 1, so we expect about 10 blocks)
        tokio::time::sleep(Duration::from_secs(10)).await;

        // wait for a certain number of blocks to be mined
        let block = provider.get_block_number().await.unwrap();
        println!("block num before restarting geth: {block}");
        assert!(block > U64::zero());

        // TODO: start rest of reth - so we can test syncing
        // can we make better test utils to simplify some of the initialization code?
        // and reduce some of the reuse?

        // === create a status ===

        // get genesis hash
        let genesis_block =
            provider.get_block(0).await.unwrap().expect("a genesis block should exist");
        let genesis_hash = genesis_block.hash.unwrap();

        // build the forkhash

        // === network ===

        let secret_key = SecretKey::new(&mut rand::thread_rng());

        let (reth_p2p, reth_disc) = unused_tcp_udp();
        let config = NetworkConfig::builder(Arc::new(NoopProvider::default()), secret_key)
            .listener_addr(reth_p2p)
            .discovery_addr(reth_disc)
            .build();
        let network = NetworkManager::new(config).await.unwrap();

        let handle = network.handle().clone();
        tokio::task::spawn(network);

        // create networkeventstream to get the next session established event easily
        let mut events = handle.event_listener();
        let geth_p2p_port = instance.p2p_port().unwrap();
        let geth_socket = SocketAddr::new([127, 0, 0, 1].into(), geth_p2p_port);

        // === ensure p2p is active ===

        // get the peer id we should be expecting
        let geth_peer_id: PeerId = enr_to_peer_id(provider.node_info().await.unwrap().enr);

        // add geth as a peer then wait for `PeerAdded` and `SessionEstablished` events.
        handle.add_peer(geth_peer_id, geth_socket);

        match events.next().await {
            Some(NetworkEvent::PeerAdded(peer_id)) => assert_eq!(peer_id, geth_peer_id),
            _ => panic!("Expected a peer added event"),
        }

        if let Some(NetworkEvent::SessionEstablished { peer_id, .. }) = events.next().await {
            assert_eq!(peer_id, geth_peer_id);
        } else {
            panic!("Expected a session established event");
        }
    })
    .await
    .unwrap();
}
