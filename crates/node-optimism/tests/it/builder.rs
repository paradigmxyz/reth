//! Node builder setup tests.

use reth_db::test_utils::create_test_rw_db;
use reth_node_builder::{components::FullNodeComponents, NodeBuilder, NodeConfig};
use reth_node_optimism::node::OptimismNode;

#[test]
fn test_basic_setup() {
    // parse CLI -> config
    let config = NodeConfig::test();
    let db = create_test_rw_db();
    let _builder = NodeBuilder::new(config)
        .with_database(db)
        .with_types(OptimismNode::default())
        .with_components(OptimismNode::components(Default::default()))
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
