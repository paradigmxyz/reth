// Talking to the DB
use reth_db::open_db_read_only;
use reth_primitives::ChainSpecBuilder;
use reth_provider::{providers::BlockchainProvider, ProviderFactory};

// Bringing up the RPC
use reth_rpc_builder::{
    RethRpcModule, RpcModuleBuilder, RpcServerConfig, TransportRpcModuleConfig,
};

// Code which we'd ideally like to not need to import if you're only spinning up
// read-only parts of the API and do not require access to pending state or to
// EVM sims
use reth_beacon_consensus::BeaconConsensus;
use reth_blockchain_tree::{
    BlockchainTree, BlockchainTreeConfig, ShareableBlockchainTree, TreeExternals,
};
use reth_revm::Factory as ExecutionFactory;
// Configuring the network parts, ideally also wouldn't ned to think about this.
use reth_network_api::test_utils::NoopNetwork;
use reth_provider::test_utils::TestCanonStateSubscriptions;
use reth_tasks::TokioTaskExecutor;
use reth_transaction_pool::test_utils::testing_pool;

use std::{path::Path, sync::Arc};

// Example illustrating how to run the ETH JSON RPC API as standalone over a DB file.
// TODO: Add example showing how to spin up your own custom RPC namespace alongside
// the other default name spaces.
#[tokio::main]
async fn main() -> eyre::Result<()> {
    // 1. Setup the DB
    let db = Arc::new(open_db_read_only(&Path::new(&std::env::var("RETH_DB_PATH")?), None)?);
    let spec = Arc::new(ChainSpecBuilder::mainnet().build());
    let factory = ProviderFactory::new(db.clone(), spec.clone());

    // 2. Setup blcokchain tree to be able to receive live notifs
    // TODO: Make this easier to configure
    let provider = {
        let consensus = Arc::new(BeaconConsensus::new(spec.clone()));
        let exec_factory = ExecutionFactory::new(spec.clone());

        let externals = TreeExternals::new(db.clone(), consensus, exec_factory, spec.clone());
        let tree_config = BlockchainTreeConfig::default();
        let (canon_state_notification_sender, _receiver) =
            tokio::sync::broadcast::channel(tree_config.max_reorg_depth() as usize * 2);

        let tree = ShareableBlockchainTree::new(BlockchainTree::new(
            externals,
            canon_state_notification_sender.clone(),
            tree_config,
        )?);

        BlockchainProvider::new(factory, tree)?
    };

    let noop_pool = testing_pool();
    let rpc_builder = RpcModuleBuilder::default()
        .with_provider(provider)
        // Rest is just defaults
        // TODO: How do we make this easier to configure?
        .with_pool(noop_pool)
        .with_network(NoopNetwork)
        .with_executor(TokioTaskExecutor::default())
        .with_events(TestCanonStateSubscriptions::default());

    // Pick which namespaces to expose.
    let config = TransportRpcModuleConfig::default().with_http([RethRpcModule::Eth]);
    let server = rpc_builder.build(config);

    // Start the server & keep it alive
    let server_args =
        RpcServerConfig::http(Default::default()).with_http_address("0.0.0.0:8545".parse()?);
    let _handle = server_args.start(server).await?;
    futures::future::pending::<()>().await;

    Ok(())
}
