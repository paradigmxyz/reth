//! Node builder setup tests.

use reth_db::test_utils::create_test_rw_db;
use reth_node_builder::{components::FullNodeComponents, NodeBuilder, NodeConfig};
use reth_node_ethereum::node::EthereumNode;

#[test]
fn test_basic_setup() {
    // parse CLI -> config
    let config = NodeConfig::test();
    let db = create_test_rw_db();
    let msg = "On components".to_string();
    let builder = NodeBuilder::new(config)
        .with_database(db)
        .with_types(EthereumNode::default())
        .with_components(EthereumNode::components())
        .on_component_initialized(move |ctx| {
            let provider = ctx.provider();
            println!("{msg}");
            Ok(())
        })
        .on_node_started(|ctx| {
            println!("Node started");
            Ok(())
        })
        .extend_rpc_modules(|ctx| {
            let _ = ctx.config();
            let _ = ctx.node().provider();

            Ok(())
        })
        .check_launch();
}
