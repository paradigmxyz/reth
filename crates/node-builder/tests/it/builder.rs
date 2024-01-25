//! Node builder setup tests.

use reth_db::test_utils::create_test_rw_db;
use reth_node_builder::{components::FullNodeComponents, ethereum::EthereumNode, NodeBuilder};
use reth_node_core::node_config::NodeConfig;

#[test]
fn test_basic_setup() {
    let config = NodeConfig::test();
    let db = create_test_rw_db();
    let builder = NodeBuilder::new(config)
        .with_database(db)
        .with_types::<EthereumNode>()
        .with_components(EthereumNode::components().map_pool(|pool| pool))
        .map_components(|components| components.map_network(|network| network))
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
