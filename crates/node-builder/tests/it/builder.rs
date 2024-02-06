//! Node builder setup tests.

use reth_db::test_utils::create_test_rw_db;
use reth_node_builder::{components::FullNodeComponents, ethereum::EthereumNode, NodeBuilder};
use reth_node_core::node_config::NodeConfig;

#[test]
fn test_basic_setup() {
    // parse CLI -> config
    let config = NodeConfig::test();
    let db = create_test_rw_db();
    let builder = NodeBuilder::new(config)
        .with_database(db)
        .with_types::<EthereumNode>()
        .with_components(EthereumNode::components())
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
