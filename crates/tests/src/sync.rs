use crate::{
    clique::CliqueGethBuilder,
    reth_builder::{RethBuilder, RethTestInstance},
};
use ethers_core::types::{
    transaction::eip2718::TypedTransaction, Eip1559TransactionRequest, H160, U64,
};
use ethers_providers::Middleware;
use reth_cli_utils::init::init_db;
use reth_consensus::BeaconConsensus;
use reth_db::mdbx::{Env, WriteMap};
use reth_network::{
    test_utils::{unused_tcp_udp, NetworkEventStream, GETH_TIMEOUT},
    NetworkConfig, NetworkManager,
};
use reth_primitives::{ChainSpec, Hardfork, Header, PeerId, SealedHeader, constants::EIP1559_INITIAL_BASE_FEE};
use reth_provider::test_utils::NoopProvider;
use secp256k1::SecretKey;
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio::fs;

/// Integration tests for the full sync pipeline.
///
/// Tests that are run against a real `geth` node use geth's Clique functionality to create blocks.
#[tokio::test(flavor = "multi_thread")]
async fn sync_from_clique_geth() {
    reth_tracing::init_test_tracing();
    tokio::time::timeout(GETH_TIMEOUT, async move {
        // first create a signer that we will fund so we can make transactions
        let chain_id = 13337u64;
        let data_dir = tempfile::tempdir().expect("should be able to create temp geth datadir");
        let dir_path = data_dir.into_path();

        // this creates a funded geth
        let clique_geth = CliqueGethBuilder::new()
            .chain_id(chain_id)
            .data_dir(dir_path.to_str().unwrap().into());

        // build the funded geth
        let mut clique_instance = clique_geth.build().await;

        // print the logs in a new task
        clique_instance.print_logs();

        // get geth to start producing blocks - use a blank password
        clique_instance.enable_mining("".into()).await;

        // === check that we have the same genesis hash ===

        // get the chainspec from the genesis we configured for geth
        let chainspec: ChainSpec = clique_instance.genesis.clone().into();
        let remote_genesis = SealedHeader::from(clique_instance.genesis().await);

        let mut local_genesis_header = Header::from(chainspec.genesis().clone());

        let hardforks = chainspec.hardforks();

        // set initial base fee depending on eip-1559
        if Some(&0u64) == hardforks.get(&Hardfork::London) {
            local_genesis_header.base_fee_per_gas = Some(EIP1559_INITIAL_BASE_FEE);
        }

        let local_genesis = local_genesis_header.seal();
        assert_eq!(local_genesis, remote_genesis, "genesis blocks should match, we computed {local_genesis:#?} but geth computed {remote_genesis:#?}");

        // === create many blocks ===

        let nonces = 0..1000u64;
        let txs = nonces
            .map(|nonce| {
                // create a tx that just sends to the zero addr
                TypedTransaction::Eip1559(Eip1559TransactionRequest::new()
                    .to(H160::zero())
                    .value(1u64)
                    .nonce(nonce))
            });

        // finally send the txs to geth
        clique_instance.send_requests(txs).await;

        // wait for a certain number of blocks to be mined
        let block = clique_instance.provider.get_block_number().await.unwrap();
        assert!(block > U64::zero());

        // get the current tip hash for pipeline configuration
        let tip_hash = clique_instance.tip_hash().await;

        // === initialize reth networking stack ===

        let secret_key = SecretKey::new(&mut rand::thread_rng());
        let (reth_p2p, reth_disc) = unused_tcp_udp();

        // may not need to set up a hello as we should be advertising compatible capabilities and
        // protocol versions anyways
        // TODO: it's sort of redundant setting BOTH the status and the genesis hash, since they
        // both contain the genesis hash. a discrepancy is probably an error. could we enforce this
        // somethow?
        let config = NetworkConfig::<Arc<NoopProvider>>::builder(secret_key)
            .listener_addr(reth_p2p)
            .discovery_addr(reth_disc)
            .status(clique_instance.status)
            .build(Arc::new(NoopProvider::default()));

        let network = NetworkManager::new(config).await.unwrap();
        let handle = network.handle().clone();

        // initialize db
        let reth_temp_dir = tempfile::tempdir().expect("should be able to create reth data dir");
        let db = Arc::new(init_db(reth_temp_dir.path()).unwrap());

        // initialize consensus
        let consensus = Arc::new(BeaconConsensus::new(chainspec.clone()));

        // build reth and start the pipeline
        let reth: RethTestInstance<Env<WriteMap>> = RethBuilder::new()
            .db(db)
            .consensus(consensus)
            .chain_spec(chainspec)
            .network(handle.clone())
            .tip(tip_hash)
            .build();

        // start reth then manually connect geth
        let pipeline_handle = tokio::task::spawn(async move { reth.start().await });
        tokio::task::spawn(network);

        // create networkeventstream to get the next session established event easily
        let mut events = NetworkEventStream::new(handle.event_listener());
        let geth_p2p_port = clique_instance.p2p_port();
        let geth_socket = SocketAddr::new([127, 0, 0, 1].into(), geth_p2p_port);

        // === ensure p2p is active ===

        // get the peer id we should be expecting
        let geth_peer_id: PeerId = clique_instance.peer_id().await;

        // add geth as a peer then wait for `PeerAdded` and `SessionEstablished` events.
        handle.add_peer(geth_peer_id, geth_socket);

        // wait for the session to be established
        let _peer_id = events.peer_added_and_established().await.unwrap();

        pipeline_handle.await.unwrap().unwrap();

        // cleanup (delete the data_dir at dir_path)
        fs::remove_dir_all(dir_path).await.unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn geth_clique_keepalive() {
    reth_tracing::init_test_tracing();
    tokio::time::timeout(GETH_TIMEOUT, async move {
        // first create a signer that we will fund so we can make transactions
        let chain_id = 13337u64;
        let data_dir = tempfile::tempdir().expect("should be able to create temp geth datadir");
        let dir_path = data_dir.into_path();

        // this creates a funded geth
        let clique_geth = CliqueGethBuilder::new()
            .chain_id(chain_id)
            .data_dir(dir_path.to_str().unwrap().into());

        // build the funded geth
        let mut clique_instance = clique_geth.build().await;

        // print the logs in a new task
        clique_instance.print_logs();

        // get geth to start producing blocks - use a blank password
        clique_instance.enable_mining("".into()).await;

        // === check that we have the same genesis hash ===

        // get the chainspec from the genesis we configured for geth
        let chainspec: ChainSpec = clique_instance.genesis.clone().into();
        let remote_genesis = SealedHeader::from(clique_instance.genesis().await);

        let mut local_genesis_header = Header::from(chainspec.genesis().clone());

        let hardforks = chainspec.hardforks();

        // set initial base fee depending on eip-1559
        if Some(&0u64) == hardforks.get(&Hardfork::London) {
            local_genesis_header.base_fee_per_gas = Some(EIP1559_INITIAL_BASE_FEE);
        }

        let local_genesis = local_genesis_header.seal();
        assert_eq!(local_genesis, remote_genesis, "genesis blocks should match, we computed {local_genesis:#?} but geth computed {remote_genesis:#?}");

        // === create many blocks ===

        let nonces = 0..1000u64;
        let txs = nonces
            .map(|nonce| {
                // create a tx that just sends to the zero addr
                TypedTransaction::Eip1559(Eip1559TransactionRequest::new()
                    .to(H160::zero())
                    .value(1u64)
                    .nonce(nonce))
            });

        // finally send the txs to geth
        clique_instance.send_requests(txs).await;

        // wait for a certain number of blocks to be mined
        let block = clique_instance.provider.get_block_number().await.unwrap();
        assert!(block > U64::zero());

        // === initialize reth networking stack ===

        let secret_key = SecretKey::new(&mut rand::thread_rng());
        let (reth_p2p, reth_disc) = unused_tcp_udp();

        // may not need to set up a hello as we should be advertising compatible capabilities and
        // protocol versions anyways
        // TODO: it's sort of redundant setting BOTH the status and the genesis hash, since they
        // both contain the genesis hash. a discrepancy is probably an error. could we enforce this
        // somethow?
        let config = NetworkConfig::<Arc<NoopProvider>>::builder(secret_key)
            .listener_addr(reth_p2p)
            .discovery_addr(reth_disc)
            .status(clique_instance.status)
            .build(Arc::new(NoopProvider::default()));

        let network = NetworkManager::new(config).await.unwrap();
        let handle = network.handle().clone();

        tokio::task::spawn(network);

        // create networkeventstream to get the next session established event easily
        let mut events = NetworkEventStream::new(handle.event_listener());
        let geth_p2p_port = clique_instance.p2p_port();
        let geth_socket = SocketAddr::new([127, 0, 0, 1].into(), geth_p2p_port);

        // === ensure p2p is active ===

        // get the peer id we should be expecting
        let geth_peer_id: PeerId = clique_instance.peer_id().await;

        // add geth as a peer then wait for `PeerAdded` and `SessionEstablished` events.
        handle.add_peer(geth_peer_id, geth_socket);

        // wait for the session to be established
        let _peer_id = events.peer_added_and_established().await.unwrap();

        // wait for session to be closed OR the duration passes
        tokio::select!(
            _ = events.next_session_closed() => {
                panic!("session closed before keepalive timeout");
            },
            _ = tokio::time::sleep(Duration::from_secs(10)) => {}
        );

        // cleanup (delete the data_dir at dir_path)
        fs::remove_dir_all(dir_path).await.unwrap();
    })
    .await
    .unwrap();
}
