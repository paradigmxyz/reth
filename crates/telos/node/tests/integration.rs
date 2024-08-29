use antelope::api::client::{APIClient, DefaultProvider};
use reth::{
    args::RpcServerArgs,
    builder::{NodeBuilder, NodeConfig},
    tasks::{TaskManager},
};
use reth_chainspec::{ChainSpecBuilder, TEVMTESTNET};
use reth_e2e_test_utils::node::NodeTestContext;
use reth_node_ethereum::EthereumNode;
use reth_primitives::{Genesis, B256};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use telos_consensus_client::client::{ConsensusClient, Error};
use telos_consensus_client::config::AppConfig;
use testcontainers::core::ContainerPort::Tcp;
use testcontainers::{runners::AsyncRunner, ContainerAsync, GenericImage};
use tracing::info;

struct TelosRethNodeHandle {
    execution_port: u16,
    jwt_secret: String,
}

async fn start_ship() -> ContainerAsync<GenericImage> {
    // Change this container to a local image if using new ship data,
    //   then make sure to update the ship data in the testcontainer-nodeos-evm repo and build a new version

    // The tag for this image needs to come from the Github packages UI, under the "OS/Arch" tab
    //   and should be the tag for linux/amd64
    let container: ContainerAsync<GenericImage> = GenericImage::new(
        "ghcr.io/telosnetwork/testcontainer-nodeos-evm",
        "v0.1.4@sha256:a8dc857e46404d74b286f8c8d8646354ca6674daaaf9eb6f972966052c95eb4a",
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
    // Chain spec with test allocs
    let genesis: Genesis =
        serde_json::from_str(include_str!("../../../ethereum/node/tests/assets/genesis.json"))
            .unwrap();
    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(TEVMTESTNET.chain)
            .genesis(genesis)
            .cancun_activated()
            .build(),
    );

    let mut rpc_config = RpcServerArgs::default().with_unused_ports().with_http();
    rpc_config.auth_jwtsecret = Some(PathBuf::from("tests/assets/jwt.hex"));

    //let _jwt = rpc_config.auth_server_config(JwtSecret::random());
    // Node setup
    let node_config = NodeConfig::test().with_chain(chain_spec).with_rpc(rpc_config.clone());

    let jwt = fs::read_to_string(node_config.rpc.auth_jwtsecret.clone().unwrap())?;
    Ok((node_config, jwt))
}

async fn start_consensus(reth_handle: TelosRethNodeHandle, ship_port: u16, chain_port: u16) -> eyre::Result<(), Error> {
    let config = AppConfig {
        log_level: "debug".to_string(),
        chain_id: 41,
        execution_endpoint: format!("http://localhost:{}", reth_handle.execution_port),
        jwt_secret: reth_handle.jwt_secret,
        ship_endpoint: format!("ws://localhost:{ship_port}"),
        chain_endpoint: format!("http://localhost:{chain_port}"),
        batch_size: 5,
        block_delta: None,
        prev_hash: B256::ZERO.to_string(),
        validate_hash: None,
        start_block: 0,
        stop_block: Some(60),
    };

    let mut client_under_test = ConsensusClient::new(config).await;
    client_under_test.run().await
}

#[tokio::test]
async fn testing_chain_sync() {
    tracing_subscriber::fmt::init();

    let container = start_ship().await;
    let chain_port = container.get_host_port_ipv4(8888).await.unwrap();
    let ship_port = container.get_host_port_ipv4(18999).await.unwrap();

    let (node_config, jwt_secret) = init_reth().unwrap();

    let exec = TaskManager::current();
    let exec = exec.executor();

    reth_tracing::init_test_tracing();

    let node_handle = NodeBuilder::new(node_config.clone())
        .testing_node(exec)
        .node(EthereumNode::default())
        .launch()
        .await
        .unwrap();

    let execution_port = node_handle.node.auth_server_handle().local_addr().port();
    let reth_handle = TelosRethNodeHandle { execution_port, jwt_secret };
    _ = NodeTestContext::new(node_handle.node.clone()).await.unwrap();
    info!("Started Reth!");

    if let Err(error) = start_consensus(reth_handle, ship_port, chain_port).await {
	panic!("Error with consensus client: {error:?}");
    }
}
