use std::{path::Path, sync::Arc};
use reth::{
    api::NodeTypesWithDBAdapter,
    beacon_consensus::EthBeaconConsensus,
    providers::{providers::{BlockchainProvider, StaticFileProvider}, ProviderFactory},
    rpc::eth::EthApi,
    utils::open_db_read_only,
};
use reth_chainspec::ChainSpecBuilder;
use reth_db::{mdbx::DatabaseArguments, ClientVersion, DatabaseEnv};
use reth::rpc::builder::{RethRpcModule, RpcModuleBuilder, RpcServerConfig, TransportRpcModuleConfig};
use myrpc_ext::{MyRpcExt, MyRpcExtApiServer};
use reth::{blockchain_tree::noop::NoopBlockchainTree, tasks::TokioTaskExecutor};
use reth_node_ethereum::{node::EthereumEngineValidator, EthEvmConfig, EthExecutorProvider, EthereumNode};
use reth_provider::{test_utils::TestCanonStateSubscriptions, ChainSpecProvider};

pub mod myrpc_ext;

#[tokio::main]
async fn main() -> eyre::Result<()> {
    // 1. Setup the DB
    let db_path = Path::new(&std::env::var("RETH_DB_PATH")?);
    let db = Arc::new(open_db_read_only(
        db_path.join("db"),
        DatabaseArguments::new(ClientVersion::default()),
    )?);

    // 2. Configure ChainSpec and ProviderFactory
    let spec = Arc::new(ChainSpecBuilder::mainnet().build());
    let factory = ProviderFactory::<NodeTypesWithDBAdapter<EthereumNode, Arc<DatabaseEnv>>>::new(
        db.clone(),
        spec.clone(),
        StaticFileProvider::read_only(db_path.join("static_files"), true)?,
    );

    // 3. Setup the blockchain provider with NoopBlockchainTree
    let provider = BlockchainProvider::new(factory, Arc::new(NoopBlockchainTree::default()))?;

    // 4. Build the RPC server with additional extensions
    let rpc_builder = RpcModuleBuilder::default()
        .with_provider(provider.clone())
        .with_noop_pool()
        .with_noop_network()
        .with_executor(TokioTaskExecutor::default())
        .with_evm_config(EthEvmConfig::new(spec.clone()))
        .with_events(TestCanonStateSubscriptions::default())
        .with_block_executor(EthExecutorProvider::ethereum(provider.chain_spec()))
        .with_consensus(EthBeaconConsensus::new(spec.clone()));

    // 5. Configure the transport module for the RPC server
    let config = TransportRpcModuleConfig::default().with_http([RethRpcModule::Eth]);
    let mut server = rpc_builder.build(
        config,
        Box::new(EthApi::with_spawner),
        Arc::new(EthereumEngineValidator::new(spec)),
    );

    // 6. Add the custom RPC extension
    let custom_rpc = MyRpcExt { provider };
    server.merge_configured(custom_rpc.into_rpc())?;

    // 7. Start the RPC server
    let server_args = RpcServerConfig::http(Default::default()).with_http_address("0.0.0.0:8545".parse()?);
    let _handle = server_args.start(&server).await?;

    // 8. Await forever
    futures::future::pending::<()>().await;

    Ok(())
}
