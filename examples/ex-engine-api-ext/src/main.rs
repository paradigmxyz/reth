//! Example demonstrating how to access the `EngineApi` instance using callback wrappers.
//!
//! Run with
//!
//! ```sh
//! cargo run -p example-ex-engine-api-ext
//! ```

use reth_db::test_utils::create_test_rw_db;
use reth_node_api::{FullNodeComponents, NodeTypesWithDBAdapter};
use reth_node_builder::{EngineApiBuilderExt, EngineApiFn, EngineNodeLauncher, LaunchNode, NodeBuilder, NodeConfig};
use reth_optimism_chainspec::BASE_MAINNET;
use reth_optimism_node::{
    args::RollupArgs,
    node::{OpAddOns, OpEngineValidatorBuilder},
    OpEngineApiBuilder, OpNode,
};
use reth_provider::providers::BlockchainProvider;
use reth_tasks::TaskManager;
use tokio::sync::oneshot;

#[tokio::main]
async fn main() {

    // Op node configuration and setup
    let config = NodeConfig::new(BASE_MAINNET.clone());
    let db = create_test_rw_db();
    let args = RollupArgs::default();
    let op_node = OpNode::new(args);
    let tasks = TaskManager::current();

    let (engine_api_tx, engine_api_rx) = oneshot::channel();

    let engine_api = EngineApiFn::new(OpEngineApiBuilder::<OpEngineValidatorBuilder>::default() , move |api| {
        let _ = engine_api_tx.send(api);
    });

    let _builder = NodeBuilder::new(config)
        .with_database(db)
        .with_types::<OpNode>()
        .with_components(op_node.components())
        .with_add_ons(OpAddOns::default().rpc_add_ons.with_engine_api(engine_api))
        .check_launch();
}
