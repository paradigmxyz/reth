//! Node builder setup tests.

use reth_db::test_utils::create_test_rw_db;
use reth_node_api::FullNodeComponents;
use reth_node_builder::{NodeBuilder, NodeConfig};
use reth_optimism_chainspec::BASE_MAINNET;
use reth_optimism_node::{node::OptimismAddOns, OptimismNode};

#[test]
fn test_basic_setup() {
    // parse CLI -> config
    let config = NodeConfig::new(BASE_MAINNET.clone());
    let db = create_test_rw_db();
    let _builder = NodeBuilder::new(config)
        .with_database(db)
        .with_types::<OptimismNode>()
        .with_components(OptimismNode::components(Default::default()))
        .with_add_ons(OptimismAddOns::new(None))
        .on_component_initialized(move |ctx| {
            let _provider = ctx.provider();
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
