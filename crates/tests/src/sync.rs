use ethers_core::{
    types::{Address, Block, Bytes, U64},
    utils::{ChainConfig, CliqueConfig, Genesis, GenesisAccount, Geth},
};
use ethers_middleware::SignerMiddleware;
use futures::StreamExt;
use reth_discv4::{bootnodes::mainnet_nodes, Discv4Config};
use reth_eth_wire::{DisconnectReason, EthVersion, Status};
use reth_interfaces::{
    p2p::headers::client::{HeadersClient, HeadersRequest},
    sync::{SyncState, SyncStateUpdater},
};
use reth_net_common::ban_list::BanList;
use reth_network::{NetworkConfig, NetworkEvent, NetworkHandle, NetworkManager, PeersConfig};
use reth_primitives::{
    proofs::genesis_state_root, Chain, ForkHash, ForkId, Header, HeadersDirection, NodeRecord,
    PeerId, H160, H256, INITIAL_BASE_FEE,
};
use reth_provider::test_utils::NoopProvider;
use secp256k1::SecretKey;
use std::{
    collections::{HashMap, HashSet},
    io::{BufRead, BufReader},
    net::SocketAddr,
    sync::Arc,
    time::Duration,
};
use tokio::task;

/// Integration tests for the full sync pipeline.
///
/// Tests that are run against a real `geth` node use geth's Clique functionality to create blocks.

#[tokio::test(flavor = "multi_thread")]
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
        let geth = geth.genesis(genesis.clone()).chain_id(chain_id).disable_discovery().insecure_unlock();

        // geth starts in dev mode, we can spawn it, mine blocks, and shut it down
        // we need to clone it because we will be reusing the geth config when we restart p2p
        let mut instance = geth.spawn();

        // set up ethers provider
        let geth_endpoint = SocketAddr::new([127, 0, 0, 1].into(), instance.port()).to_string();
        let provider = Provider::<Http>::try_from(format!("http://{geth_endpoint}")).unwrap();
        let provider =
            SignerMiddleware::new_with_provider_chain(provider, wallet.clone()).await.unwrap();

        // === start geth and produce blocks ===

        // first get the balance and make sure its not zero
        let balance = provider.get_balance(our_address, None).await.unwrap();
        assert_ne!(balance, 0u64.into());
        println!("address: {our_address:?}");
        println!("balance at genesis: {balance:?}");

        // TODO: create transactions

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

        // TODO: remove
        // wait for stuff to happen (we are using period 1, so we expect about 10 blocks)
        tokio::time::sleep(Duration::from_secs(5)).await;

        // wait for a certain number of blocks to be mined
        let block = provider.get_block_number().await.unwrap();
        println!("block num before restarting geth: {block}");
        assert!(block > U64::zero());

        // get genesis hash
        let genesis_block =
            provider.get_block(0).await.unwrap().expect("a genesis block should exist");

        // get our hash
        let sealed_genesis = genesis_header(&genesis.clone()).seal();

        // let's just convert into a reth header and compare
        let geth_genesis_header = block_to_header(genesis_block.clone()).seal();
        assert_eq!(geth_genesis_header, sealed_genesis, "genesis headers should match, we computed {sealed_genesis:#?} but geth computed {geth_genesis_header:#?}");

        // make sure we have the same hash
        let genesis_hash: H256 = genesis_block.hash.unwrap().0.into();
        let sealed_hash = sealed_genesis.hash();
        assert_eq!(sealed_hash, genesis_hash, "genesis hashes should match, we computed {sealed_hash:?} but geth computed {genesis_hash:?}");

        // === initialize reth networking stack ===

        let secret_key = SecretKey::new(&mut rand::thread_rng());
        let (reth_p2p, reth_disc) = unused_tcp_udp();

        // get correct status using genesis information
        let status = extract_status(&genesis);

        // may not need to set up a hello as we should be advertising compatible capabilities and
        // protocol versions anyways
        // TODO: it's sort of redundant setting BOTH the status and the genesis hash, since they
        // both contain the genesis hash. a discrepancy is probably an error. could we enforce this
        // somethow?
        let config = NetworkConfig::builder(Arc::new(NoopProvider::default()), secret_key)
            .listener_addr(reth_p2p)
            .discovery_addr(reth_disc)
            .genesis_hash(sealed_genesis.hash())
            .status(status)
            .build();
        let network = NetworkManager::new(config).await.unwrap();

        let handle = network.handle().clone();

        // start reth then manually connect geth
        tokio::task::spawn(start_reth(handle.clone()));
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

        match events.next().await.expect("the network should emit at least one event") {
            // TODO: should the first event be a peeradded, and the second to be a
            // sessionestablished?
            // or should we just wait for a PeerAdded and SessionEstablished event?
            // I guess, is there any relationship between the event variants and their ordering?
            NetworkEvent::SessionEstablished { peer_id, .. } => assert_eq!(peer_id, geth_peer_id),
            NetworkEvent::SessionClosed { peer_id, reason } => {
                panic!("Expected a session established event, got a session closed event instead: {peer_id:?} {reason:?}")
            },
            NetworkEvent::PeerAdded(peer_id) => {
                assert_eq!(peer_id, geth_peer_id);
            },
            NetworkEvent::PeerRemoved(peer_id) => {
                panic!("The first event from the network should not be a peer removed event: {peer_id:?}")
            },
        }

        // see TODO above ^. assuming the first event we get is a PeerAdded event, the second
        // should be a SessionEstablished event
        // TODO: these matches seem not great, also can we log the pending session authentication
        // error here somehow?
        match events.next().await.expect("the network should emit a second event") {
            NetworkEvent::SessionEstablished { peer_id, .. } => assert_eq!(peer_id, geth_peer_id),
            NetworkEvent::SessionClosed { peer_id, reason } => {
                panic!("Expected a session established event, got a session closed event instead: {peer_id:?} {reason:?}")
            },
            NetworkEvent::PeerAdded(peer_id) => {
                panic!("The second event from the network should not be a peer added event: {peer_id:?}")
            },
            NetworkEvent::PeerRemoved(peer_id) => {
                panic!("The second event from the network should not be a peer removed event: {peer_id:?}")
            },
        }

        // what kinds of events are we waiting for from reth? what kinds of assertions should we
        // make?
    })
    .await
    .unwrap();
}
