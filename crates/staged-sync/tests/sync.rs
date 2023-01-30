use ethers_core::{
    types::{transaction::eip2718::TypedTransaction, Eip1559TransactionRequest, H160, U64},
    utils::Geth,
};
use ethers_providers::Middleware;
use reth_db::mdbx::{Env, WriteMap};
use reth_downloaders::{
    bodies::concurrent::ConcurrentDownloaderBuilder, headers::linear::LinearDownloadBuilder,
};
use reth_interfaces::{sync::NoopSyncStateUpdate, test_utils::TestConsensus};
use reth_network::{
    test_utils::{unused_tcp_udp, NetworkEventStream, GETH_TIMEOUT},
    NetworkConfig, NetworkManager,
};
use reth_primitives::{
    constants::EIP1559_INITIAL_BASE_FEE, ChainSpec, ForkKind, Hardfork, Header, PeerId,
    SealedHeader, H256,
};
use reth_provider::test_utils::NoopProvider;
use reth_staged_sync::{
    test_utils::{CliqueGethInstance, CliqueMiddleware},
    utils::init::init_db,
};
use reth_stages::{sets::DefaultStages, Pipeline};
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
        let chain_id = 13338u64;
        let data_dir = tempfile::tempdir().expect("should be able to create temp geth datadir");
        let dir_path = data_dir.into_path();
        tracing::info!(
            data_dir=?dir_path,
            "initializing geth instance"
        );

        // this creates a funded geth
        let clique_geth = Geth::new()
            .chain_id(chain_id)
            .data_dir(dir_path.to_str().unwrap().into());

        // build the funded geth
        let (mut clique, provider) = CliqueGethInstance::new(clique_geth, None).await;
        let geth_p2p_port = clique.0.p2p_port().expect("geth should be configured with a p2p port");
        tracing::info!(
            p2p_port=%geth_p2p_port,
            rpc_port=%clique.0.port(),
            "configured clique geth instance in sync test"
        );

        // don't print logs, but drain the stderr
        clique.prevent_blocking().await;

        // get geth to start producing blocks - use a blank password
        provider.enable_mining(clique.0.clique_private_key(), "".into()).await;
        tracing::info!("enabled block production");

        // === check that we have the same genesis hash ===

        // get the chainspec from the genesis we configured for geth
        let mut chainspec: ChainSpec = clique.0.genesis().clone().into();
        let remote_genesis = SealedHeader::from(provider.remote_genesis_block().await.unwrap());

        let mut local_genesis_header: Header = chainspec.genesis().clone().into();

        let hardforks = chainspec.hardforks();

        // set initial base fee depending on eip-1559
        if let Some(ForkKind::Block(0)) = hardforks.get(&Hardfork::London) {
            local_genesis_header.base_fee_per_gas = Some(EIP1559_INITIAL_BASE_FEE);
        }

        let local_genesis = local_genesis_header.seal();
        assert_eq!(local_genesis, remote_genesis, "genesis blocks should match, we computed {local_genesis:#?} but geth computed {remote_genesis:#?}");

        // set the chainspec genesis hash
        chainspec.genesis_hash = local_genesis.hash();

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
        provider.send_requests(txs).await.unwrap();

        let block = provider.get_block_number().await.unwrap();
        assert!(block > U64::zero());

        // get the current tip hash for pipeline configuration
        let tip = provider.remote_tip_block().await.unwrap();
        let tip_hash: H256 = tip.hash.unwrap().0.into();

        tracing::info!(genesis_hash = ?chainspec.genesis_hash, "genesis hash");
        tracing::info!(tip_hash = ?tip_hash, "tip hash");
        tracing::info!(tip_number = ?tip.number, "tip number");

        // === initialize reth networking stack ===

        let secret_key = SecretKey::new(&mut rand::thread_rng());
        let (reth_p2p, reth_disc) = unused_tcp_udp();
        tracing::info!(
            %reth_p2p,
            %reth_disc,
            "setting up reth networking stack in sync test"
        );

        let config = NetworkConfig::<Arc<NoopProvider>>::builder(secret_key)
            .listener_addr(reth_p2p)
            .discovery_addr(reth_disc)
            .chain_spec(chainspec.clone())
            .build(Arc::new(NoopProvider::default()));

        let network = NetworkManager::new(config).await.unwrap();
        let handle = network.handle().clone();

        // initialize db
        let reth_temp_dir = tempfile::tempdir().expect("should be able to create reth data dir");
        let db = Arc::new(init_db(reth_temp_dir.path()).unwrap());

        // initialize consensus
        let consensus = Arc::new(TestConsensus::default());

        // set up downloaders
        let fetch_client = Arc::new(network.fetch_client());

        let bodies_downloader = ConcurrentDownloaderBuilder::default()
            .build(fetch_client.clone(), consensus.clone(), db.clone());
        let headers_downloader = LinearDownloadBuilder::default()
            .build(consensus.clone(), fetch_client);

        // initialize pipeline
        let mut reth: Pipeline<Env<WriteMap>, NoopSyncStateUpdate> = Pipeline::builder()
            .add_stages(DefaultStages::new(consensus, headers_downloader, bodies_downloader))
            .build();

        // start reth then manually connect geth
        let pipeline_handle = tokio::task::spawn(async move {
            reth.run(db).await.unwrap();
        });

        tokio::task::spawn(network);

        // create networkeventstream to get the next session established event easily
        let mut events = NetworkEventStream::new(handle.event_listener());
        let geth_socket = SocketAddr::new([127, 0, 0, 1].into(), geth_p2p_port);

        // === ensure p2p is active ===

        // get the peer id we should be expecting
        let geth_peer_id: PeerId = provider.peer_id().await.unwrap();

        // add geth as a peer then wait for `PeerAdded` and `SessionEstablished` events.
        handle.add_peer(geth_peer_id, geth_socket);

        // wait for the session to be established
        let _peer_id = events.peer_added_and_established().await.unwrap();

        tracing::info!("waiting for pipeline to finish");
        pipeline_handle.await.unwrap();

        drop(clique);

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
        tracing::info!(
            data_dir=?dir_path,
            "initializing geth instance"
        );

        // this creates a funded geth
        let clique_geth = Geth::new()
            .chain_id(chain_id)
            .data_dir(dir_path.to_str().unwrap().into());

        // build the funded geth
        let (mut clique, provider) = CliqueGethInstance::new(clique_geth, None).await;
        let geth_p2p_port = clique.0.p2p_port().expect("geth should be configured with a p2p port");
        tracing::info!(
            p2p_port=%geth_p2p_port,
            rpc_port=%clique.0.port(),
            "configured clique geth instance in keepalive test"
        );

        // don't print logs, but drain the stderr
        clique.prevent_blocking().await;

        // get geth to start producing blocks - use a blank password
        provider.enable_mining(clique.0.clique_private_key(), "".into()).await;

        // === check that we have the same genesis hash ===

        // get the chainspec from the genesis we configured for geth
        let mut chainspec: ChainSpec = clique.0.genesis().clone().into();
        let remote_genesis = SealedHeader::from(provider.remote_genesis_block().await.unwrap());

        let mut local_genesis_header = Header::from(chainspec.genesis().clone());

        let hardforks = chainspec.hardforks();

        // set initial base fee depending on eip-1559
        if let Some(ForkKind::Block(0)) = hardforks.get(&Hardfork::London) {
            local_genesis_header.base_fee_per_gas = Some(EIP1559_INITIAL_BASE_FEE);
        }

        let local_genesis = local_genesis_header.seal();
        assert_eq!(local_genesis, remote_genesis, "genesis blocks should match, we computed {local_genesis:#?} but geth computed {remote_genesis:#?}");

        // set the chainspec genesis hash
        chainspec.genesis_hash = local_genesis.hash();

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
        tracing::info!("generated transactions for blocks");

        // finally send the txs to geth
        provider.send_requests(txs).await;

        let block = provider.get_block_number().await.unwrap();
        assert!(block > U64::zero());

        // === initialize reth networking stack ===

        let secret_key = SecretKey::new(&mut rand::thread_rng());
        let (reth_p2p, reth_disc) = unused_tcp_udp();
        tracing::info!(
            %reth_p2p,
            %reth_disc,
            "setting up reth networking stack in keepalive test"
        );

        let config = NetworkConfig::<Arc<NoopProvider>>::builder(secret_key)
            .listener_addr(reth_p2p)
            .discovery_addr(reth_disc)
            .chain_spec(chainspec.clone())
            .build(Arc::new(NoopProvider::default()));

        let network = NetworkManager::new(config).await.unwrap();
        let handle = network.handle().clone();

        tokio::task::spawn(network);

        // create networkeventstream to get the next session established event easily
        let mut events = NetworkEventStream::new(handle.event_listener());
        let geth_socket = SocketAddr::new([127, 0, 0, 1].into(), geth_p2p_port);

        // get the peer id we should be expecting
        let geth_peer_id: PeerId = provider.peer_id().await.unwrap();

        // add geth as a peer then wait for `PeerAdded` and `SessionEstablished` events.
        handle.add_peer(geth_peer_id, geth_socket);

        // wait for the session to be established
        let _peer_id = events.peer_added_and_established().await.unwrap();

        // wait for session to be closed OR the duration passes
        let keepalive_duration = Duration::from_secs(30);
        tokio::select!(
            _ = events.next_session_closed() => {
                panic!("session closed before keepalive timeout");
            },
            _ = tokio::time::sleep(keepalive_duration) => {}
        );

        // cleanup (delete the data_dir at dir_path)
        fs::remove_dir_all(dir_path).await.unwrap();
    })
    .await
    .unwrap();
}
