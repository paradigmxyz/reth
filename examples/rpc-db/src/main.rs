//! Example illustrating how to run the ETH JSON RPC API as standalone over a DB file.
//!
//! Run with
//!
//! ```not_rust
//! cargo run -p rpc-db
//! ```
//!
//! This installs an additional RPC method `myrpcExt_customMethod` that can queried via [cast](https://github.com/foundry-rs/foundry)
//!
//! ```sh
//! cast rpc myrpcExt_customMethod
//! ```
use reth::{
    primitives::ChainSpecBuilder,
    providers::{providers::BlockchainProvider, ProviderFactory},
    utils::db::open_db_read_only,
};
// Bringing up the RPC
use reth::rpc::builder::{
    RethRpcModule, RpcModuleBuilder, RpcServerConfig, TransportRpcModuleConfig,
};

// Code which we'd ideally like to not need to import if you're only spinning up
// read-only parts of the API and do not require access to pending state or to
// EVM sims
use reth::{
    beacon_consensus::BeaconConsensus,
    blockchain_tree::{
        BlockchainTree, BlockchainTreeConfig, ShareableBlockchainTree, TreeExternals,
    },
    revm::Factory as ExecutionFactory,
};

// Configuring the network parts, ideally also wouldn't ned to think about this.
use reth::{providers::test_utils::TestCanonStateSubscriptions, tasks::TokioTaskExecutor};
use std::{path::Path, sync::Arc};

use myrpc_ext::{MyRpcExt, MyRpcExtApiServer};
// Custom rpc extension
pub mod myrpc_ext;

#[tokio::main]
async fn main() -> eyre::Result<()> {
    // 1. Setup the DB
    let db = Arc::new(open_db_read_only(Path::new(&std::env::var("RETH_DB_PATH")?), None)?);
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
            canon_state_notification_sender,
            tree_config,
            None,
        )?);

        BlockchainProvider::new(factory, tree)?
    };

    let rpc_builder = RpcModuleBuilder::default()
        .with_provider(provider.clone())
        // Rest is just noops that do nothing
        .with_noop_pool()
        .with_noop_network()
        .with_executor(TokioTaskExecutor::default())
        .with_events(TestCanonStateSubscriptions::default());

    // Pick which namespaces to expose.
    let config = TransportRpcModuleConfig::default().with_http([RethRpcModule::Eth]);
    let mut server = rpc_builder.build(config);

    // Add a custom rpc namespace
    let custom_rpc = MyRpcExt { provider };
    server.merge_configured(custom_rpc.into_rpc())?;

    // Start the server & keep it alive
    let server_args =
        RpcServerConfig::http(Default::default()).with_http_address("0.0.0.0:8545".parse()?);
    let _handle = server_args.start(server).await?;
    futures::future::pending::<()>().await;

    Ok(())
}
