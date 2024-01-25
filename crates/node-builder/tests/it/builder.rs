//! Node builder setup tests.

use reth_db::test_utils::create_test_rw_db;
use reth_node_builder::{ethereum::EthereumNode, rpc::RpcContext, NodeBuilder};
use reth_node_core::node_config::NodeConfig;

fn test_basic_setup() {
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
        .extend_rpc_modules(|ctx: RpcContext<'_, _>| {
            let _ = ctx.config();
            Ok(())
        })
        .check_launch();
}
