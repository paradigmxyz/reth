use example_bsc_p2p::{
    block_import::{service::ImportService, BscBlockImport},
    chainspec::{boot_nodes, bsc_chain_spec, head},
    handshake::BscHandshake,
};
use reth_discv4::Discv4ConfigBuilder;
use reth_engine_primitives::{BeaconConsensusEngineHandle, BeaconEngineMessage};
use reth_engine_service::service::EngineService;
use reth_engine_tree::{
    test_utils::TestPipelineBuilder,
    tree::{NoopInvalidBlockHook, TreeConfig},
};
use reth_eth_wire::{BlockHashOrNumber, GetBlockBodies, GetBlockHeaders, HeadersDirection};
use reth_ethereum_consensus::EthBeaconConsensus;
use reth_ethereum_engine_primitives::EthEngineTypes;
use reth_evm_ethereum::{execute::EthExecutorProvider, EthEvmConfig};
use reth_exex_types::FinishedExExHeight;
use reth_network::{
    p2p::test_utils::TestFullBlockClient, EthNetworkPrimitives, NetworkConfig, NetworkEvent,
    NetworkEventListenerProvider, NetworkManager, PeerRequest,
};
use reth_node_ethereum::EthereumEngineValidator;
use reth_payload_builder::PayloadBuilderHandle;
use reth_primitives::SealedHeader;
use reth_provider::{
    noop::NoopProvider, providers::BlockchainProvider,
    test_utils::create_test_provider_factory_with_chain_spec, BlockNumReader,
};
use reth_prune::Pruner;
use reth_tasks::{TaskSpawner, TokioTaskExecutor};
use secp256k1::{rand, SecretKey};
use std::{
    net::{Ipv4Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};
use tokio::sync::{mpsc::unbounded_channel, oneshot, watch};
use tokio_stream::{wrappers::UnboundedReceiverStream, StreamExt};
use tracing::info;

#[tokio::test(flavor = "multi_thread")]
async fn can_import_block() {
    reth_tracing::init_test_tracing();
    let local_addr = SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), 30303);
    let secret_key = SecretKey::new(&mut rand::thread_rng());
    let chain_spec = bsc_chain_spec();
    let consensus = Arc::new(EthBeaconConsensus::new(chain_spec.clone()));
    let provider_factory = create_test_provider_factory_with_chain_spec(chain_spec.clone());
    let blockchain_db =
        BlockchainProvider::with_latest(provider_factory.clone(), SealedHeader::default()).unwrap();
    let evm_config = EthEvmConfig::new(chain_spec.clone());
    let (_tx, rx) = watch::channel(FinishedExExHeight::NoExExs);
    let pruner = Pruner::new_with_factory(provider_factory.clone(), vec![], 0, 0, None, rx);
    let pipeline = TestPipelineBuilder::new().build(chain_spec.clone());
    let pipeline_task_spawner = Box::<TokioTaskExecutor>::default();
    let executor_factory = EthExecutorProvider::ethereum(chain_spec.clone());
    let (to_engine, engine_rx) = unbounded_channel::<BeaconEngineMessage<EthEngineTypes>>();
    let incoming_requests = UnboundedReceiverStream::new(engine_rx);
    let client = TestFullBlockClient::default();
    let (sync_metrics_tx, _sync_metrics_rx) = unbounded_channel();
    let (tx, _rx) = unbounded_channel();
    let engine_payload_validator = EthereumEngineValidator::new(chain_spec.clone());

    let mut eth_service = EngineService::new(
        consensus,
        executor_factory,
        chain_spec.clone(),
        client,
        Box::pin(incoming_requests),
        pipeline,
        pipeline_task_spawner,
        provider_factory.clone(),
        blockchain_db,
        pruner,
        PayloadBuilderHandle::new(tx),
        engine_payload_validator,
        TreeConfig::default(),
        Box::new(NoopInvalidBlockHook::default()),
        sync_metrics_tx,
        evm_config,
    );

    let executor = TokioTaskExecutor::default();
    executor.spawn(Box::pin(async move {
        loop {
            eth_service.next().await.unwrap();
        }
    }));

    let (import_service, import_handle) =
        ImportService::new(provider_factory.clone(), BeaconConsensusEngineHandle::new(to_engine));

    executor.spawn(Box::pin(async move {
        import_service.await.unwrap();
    }));

    let block_import = BscBlockImport::new(import_handle);

    let net_cfg = NetworkConfig::<_, EthNetworkPrimitives>::builder(secret_key)
        .block_import(Box::new(block_import))
        .boot_nodes(boot_nodes())
        .set_head(head())
        .with_pow()
        .listener_addr(local_addr)
        .eth_rlpx_handshake(Arc::new(BscHandshake::default()))
        .build(NoopProvider::eth(chain_spec));

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

            // Get block headers
            let (tx, rx) = oneshot::channel();
            let request = PeerRequest::GetBlockHeaders {
                request: GetBlockHeaders {
                    start_block: BlockHashOrNumber::Number(0),
                    limit: 10,
                    skip: 0,
                    direction: HeadersDirection::Rising,
                },
                response: tx,
            };
            messages.try_send(request).unwrap();
            let headers = rx.await.unwrap().unwrap();
            info!("Received headers: {:?}", headers);

            // Get block bodies
            let (tx, rx) = oneshot::channel();
            let request = PeerRequest::GetBlockBodies {
                request: GetBlockBodies(vec![headers.0[1].hash_slow()]),
                response: tx,
            };
            messages.try_send(request).unwrap();
            let bodies = rx.await.unwrap().unwrap();
            info!("Received bodies: {:?}", bodies);

            // give it some time to import the block
            tokio::time::sleep(Duration::from_secs(1)).await;
            // use the provider to see if it was imported
            let latest_block = provider_factory.best_block_number().unwrap();
            info!("Latest block: {:?}", latest_block);

            break; // Exit the loop once we've handled the ActivePeerSession
        }
    }
}
