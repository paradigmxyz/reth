//! Node builder setup tests.

use std::sync::Arc;

use reth_db::{
    test_utils::{create_test_rw_db, TempDatabase},
    DatabaseEnv,
};
use reth_node_api::NodeTypesWithDBAdapter;
use reth_node_builder::{EngineNodeLauncher, FullNodeComponents, NodeBuilder, NodeConfig};
use reth_node_ethereum::node::{EthereumAddOns, EthereumNode};
use reth_provider::providers::BlockchainProvider2;
use reth_tasks::TaskManager;

#[test]
fn test_basic_setup() {
    // parse CLI -> config
    let config = NodeConfig::test();
    let db = create_test_rw_db();
    let msg = "On components".to_string();
    let _builder = NodeBuilder::new(config)
        .with_database(db)
        .with_types::<EthereumNode>()
        .with_components(EthereumNode::components())
        .with_add_ons(EthereumAddOns::default())
        .on_component_initialized(move |ctx| {
            let _provider = ctx.provider();
            println!("{msg}");
            Ok(())
        })
        .on_node_started(|_full_node| Ok(()))
        .on_rpc_started(|_ctx, handles| {
            let _client = handles.rpc.http_client();
            Ok(())
        })
        .extend_rpc_modules(|ctx| {
            let _ = ctx.config();
            let _ = ctx.node().provider();

            Ok(())
        })
        .check_launch();
}

#[tokio::test]
async fn test_eth_launcher() {
    let tasks = TaskManager::current();
    let config = NodeConfig::test();
    let db = create_test_rw_db();
    let _builder =
        NodeBuilder::new(config)
            .with_database(db)
            .with_types_and_provider::<EthereumNode, BlockchainProvider2<
                NodeTypesWithDBAdapter<EthereumNode, Arc<TempDatabase<DatabaseEnv>>>,
            >>()
            .with_components(EthereumNode::components())
            .with_add_ons(EthereumAddOns::default())
            .launch_with_fn(|builder| {
                let launcher = EngineNodeLauncher::new(
                    tasks.executor(),
                    builder.config.datadir(),
                    Default::default(),
                );
                builder.launch_with(launcher)
            });
}

#[test]
fn test_node_setup() {
    let config = NodeConfig::test();
    let db = create_test_rw_db();
    let _builder =
        NodeBuilder::new(config).with_database(db).node(EthereumNode::default()).check_launch();
}
