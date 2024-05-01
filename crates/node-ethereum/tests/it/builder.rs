//! Node builder setup tests.

use reth_db::test_utils::create_test_rw_db;
use reth_node_api::FullNodeComponents;
use reth_node_builder::{NodeBuilder, NodeConfig};
use reth_node_ethereum::node::EthereumNode;

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

#[test]
fn test_node_setup() {
    let config = NodeConfig::test();
    let db = create_test_rw_db();
    let _builder =
        NodeBuilder::new(config).with_database(db).node(EthereumNode::default()).check_launch();
}
