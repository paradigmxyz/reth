use alloy_provider::{Provider, ProviderBuilder};
use antelope::api::client::{APIClient, DefaultProvider};
use reth::{
    args::RpcServerArgs,
    builder::{NodeBuilder, NodeConfig},
    tasks::TaskManager,
};
use reth_chainspec::{ChainSpecBuilder, TEVMTESTNET};
use reth_e2e_test_utils::node::NodeTestContext;
use reth_node_telos::{TelosArgs, TelosNode};
use reth_telos_rpc::TelosClient;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use reqwest::Url;
use telos_consensus_client::client::ConsensusClient;
use telos_consensus_client::config::{AppConfig, CliArgs};
use telos_consensus_client::data::Block;
use telos_translator_rs::{block::TelosEVMBlock, types::translator_types::ChainId};
use telos_consensus_client::main_utils::build_consensus_client;
use telos_translator_rs::translator::Translator;
use testcontainers::core::ContainerPort::Tcp;
use testcontainers::{runners::AsyncRunner, ContainerAsync, GenericImage};
use tokio::sync::mpsc;
use tokio::time::sleep;

pub mod live_test_runner;

struct TelosRethNodeHandle {
    execution_port: u16,
    jwt_secret: String,
}

const CONTAINER_TAG: &str = "v0.1.9@sha256:6d4946f112e5c26712a938ea332b76e742035e64af93149227d97dd67e1a9012";
const CONTAINER_LAST_EVM_BLOCK: u64 = 50;

async fn start_ship() -> ContainerAsync<GenericImage> {
    // Change this container to a local image if using new ship data,
    //   then make sure to update the ship data in the testcontainer-nodeos-evm repo and build a new version

    // The tag for this image needs to come from the Github packages UI, under the "OS/Arch" tab
    //   and should be the tag for linux/amd64
    let container: ContainerAsync<GenericImage> = GenericImage::new(
        "ghcr.io/telosnetwork/testcontainer-nodeos-evm", CONTAINER_TAG
    )
    .with_exposed_port(Tcp(8888))
    .with_exposed_port(Tcp(18999))
    .start()
    .await
    .unwrap();

    let port_8888 = container.get_host_port_ipv4(8888).await.unwrap();

    let api_base_url = format!("http://localhost:{port_8888}");
    let api_client = APIClient::<DefaultProvider>::default_provider(api_base_url).unwrap();

    let mut last_block = 0;

    loop {
        let Ok(info) = api_client.v1_chain.get_info().await else {
            println!("Waiting for telos node to produce blocks...");
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            continue;
        };
        if last_block != 0 && info.head_block_num > last_block {
            break;
        }
        last_block = info.head_block_num;
    }

    container
}

fn init_reth() -> eyre::Result<(NodeConfig, String)> {
    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(TEVMTESTNET.chain)
            .genesis(TEVMTESTNET.genesis.clone())
            .frontier_activated()
            .homestead_activated()
            .tangerine_whistle_activated()
            .spurious_dragon_activated()
            .byzantium_activated()
            .constantinople_activated()
            .petersburg_activated()
            .istanbul_activated()
            .berlin_activated()
            .build(),
    );

    let mut rpc_config = RpcServerArgs::default().with_unused_ports().with_http();
    rpc_config.auth_jwtsecret = Some(PathBuf::from("tests/assets/jwt.hex"));

    // Node setup
    let node_config = NodeConfig::test().with_chain(chain_spec).with_rpc(rpc_config.clone());

    let jwt = fs::read_to_string(node_config.rpc.auth_jwtsecret.clone().unwrap())?;
    Ok((node_config, jwt))
}

async fn build_consensus_and_translator(
    reth_handle: TelosRethNodeHandle,
    ship_port: u16,
    chain_port: u16,
) -> (ConsensusClient, Translator) {
    let config = AppConfig {
        log_level: "debug".to_string(),
        chain_id: ChainId(41),
        execution_endpoint: format!("http://localhost:{}", reth_handle.execution_port),
        jwt_secret: reth_handle.jwt_secret,
        ship_endpoint: format!("ws://localhost:{ship_port}"),
        chain_endpoint: format!("http://localhost:{chain_port}"),
        batch_size: 10,
        prev_hash: "b25034033c9ca7a40e879ddcc29cf69071a22df06688b5fe8cc2d68b4e0528f9".to_string(),
        validate_hash: None,
        evm_start_block: 1,
        // TODO: Determine a good stop block and test it here
        evm_stop_block: None,
        data_path: "temp/db".to_string(),
        block_checkpoint_interval: 1000,
        maximum_sync_range: 100000,
        latest_blocks_in_db_num: 100,
        max_retry: None,
        retry_interval: None,
    };

    let cli_args = CliArgs { config: "".to_string(), clean: true };

    let c = build_consensus_client(&cli_args, config).await.unwrap();
    let translator = Translator::new((&c.config).into());

    (c, translator)
}

#[tokio::test]
async fn testing_chain_sync() {
    tracing_subscriber::fmt::init();

    println!("Starting test node");
    let container = start_ship().await;
    let chain_port = container.get_host_port_ipv4(8888).await.unwrap();
    let ship_port = container.get_host_port_ipv4(18999).await.unwrap();

    let (node_config, jwt_secret) = init_reth().unwrap();

    let exec = TaskManager::current();
    let exec = exec.executor();

    reth_tracing::init_test_tracing();

    let telos_args = TelosArgs {
        telos_endpoint: Some(format!("http://localhost:{chain_port}")),
        signer_account: Some("rpc.evm".to_string()),
        signer_permission: Some("active".to_string()),
        signer_key: Some("5Jr65kdYmn33C3UabzhmWDm2PuqbRfPuDStts3ZFNSBLM7TqaiL".to_string()),
        gas_cache_seconds: None,
    };

    let node_handle = NodeBuilder::new(node_config.clone())
        .testing_node(exec)
        .node(TelosNode::new(telos_args.clone()))
        .extend_rpc_modules(move |ctx| {
            if telos_args.telos_endpoint.is_some() {
                ctx.registry.eth_api().set_telos_client(TelosClient::new(telos_args.into()));
            }

            Ok(())
        })
        .launch()
        .await
        .unwrap();

    let execution_port = node_handle.node.auth_server_handle().local_addr().port();
    let rpc_port = node_handle.node.rpc_server_handles.rpc.http_local_addr().unwrap().port();
    let reth_handle = TelosRethNodeHandle { execution_port, jwt_secret };
    println!("Starting Reth on RPC port {}!", rpc_port);
    let _ = NodeTestContext::new(node_handle.node.clone()).await.unwrap();
    println!("Starting consensus on RPC port {}!", rpc_port);
    let (client, translator) =
        build_consensus_and_translator(reth_handle, ship_port, chain_port).await;

    let consensus_shutdown = client.shutdown_handle();
    let translator_shutdown = translator.shutdown_handle();

    let (block_sender, block_receiver) = mpsc::channel::<TelosEVMBlock>(1000);

    println!("Telos consensus client starting, awaiting result...");
    let client_handle = tokio::spawn(client.run(block_receiver));

    println!("Telos translator client is starting...");
    let translator_handle = tokio::spawn(translator.launch(Some(block_sender)));

    let rpc_url = Url::from(format!("http://localhost:{}", rpc_port).parse().unwrap());
    let provider = ProviderBuilder::new().on_http(rpc_url.clone());

    // todo wait time or anything else to wait for shutdown
    println!("Waiting to send shutdown signal...");

    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let latest_block = provider.get_block_number().await.unwrap();
        println!("Latest block: {latest_block}");
        if client_handle.is_finished() {
            _ = translator_shutdown.shutdown().await.unwrap();
            break;
        }
        if latest_block > CONTAINER_LAST_EVM_BLOCK {
            _ = translator_shutdown.shutdown().await.unwrap();
            _ = consensus_shutdown.shutdown().await.unwrap();
            break;
        }
    }

    _ = tokio::join!(client_handle, translator_handle);
    println!("Translator shutdown done.");
    println!("Client shutdown done.");

    live_test_runner::run_tests(
        &rpc_url.clone().to_string(),
        "87ef69a835f8cd0c44ab99b7609a20b2ca7f1c8470af4f0e5b44db927d542084",
    )
    .await;
}
