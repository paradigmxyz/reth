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
// Configuring the network parts, ideally also wouldn't ned to think about this.
use myrpc_ext::{MyRpcExt, MyRpcExtApiServer};
use reth::{
    blockchain_tree::noop::NoopBlockchainTree, providers::test_utils::TestCanonStateSubscriptions,
    tasks::TokioTaskExecutor,
};
use reth_node_ethereum::EthEvmConfig;
use std::{path::Path, sync::Arc};

// Custom rpc extension
pub mod myrpc_ext;

#[tokio::main]
async fn main() -> eyre::Result<()> {
    // 1. Setup the DB
    let db = Arc::new(open_db_read_only(
        Path::new(&std::env::var("RETH_DB_PATH")?),
        Default::default(),
    )?);
    let spec = Arc::new(ChainSpecBuilder::mainnet().build());
    let factory = ProviderFactory::new(db.clone(), spec.clone());

    // 2. Setup the blockchain provider using only the database provider and a noop for the tree to
    //    satisfy trait bounds. Tree is not used in this example since we are only operating on the
    //    disk and don't handle new blocks/live sync etc, which is done by the blockchain tree.
    let provider = BlockchainProvider::new(factory, NoopBlockchainTree::default())?;

    let rpc_builder = RpcModuleBuilder::default()
        .with_provider(provider.clone())
        // Rest is just noops that do nothing
        .with_noop_pool()
        .with_noop_network()
        .with_executor(TokioTaskExecutor::default())
        .with_evm_config(EthEvmConfig::default())
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
