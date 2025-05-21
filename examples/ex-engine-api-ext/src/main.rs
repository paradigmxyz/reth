//! Example demonstrating how to access the Engine API instance.
//!
//! Run with
//!
//! ```sh
//! cargo run -p example-ex-engine-api-fn
//! ```

use reth_db::test_utils::create_test_rw_db;
use reth_node_api::{FullNodeComponents, NodeTypesWithDBAdapter};
use reth_node_builder::{NodeBuilder, NodeConfig};
use reth_optimism_chainspec::BASE_MAINNET;
use reth_optimism_node::{
    args::RollupArgs,
    node::{OpAddOns, OpEngineValidatorBuilder},
    OpEngineApiBuilder, OpNode,
};
use reth_provider::providers::BlockchainProvider;
use reth_rpc_api::IntoEngineApiRpcModule;
use std::sync::Arc;
use tokio::sync::oneshot;

fn main() {
    let config = NodeConfig::new(BASE_MAINNET.clone());
    let db = create_test_rw_db();
    let args = RollupArgs::default();
    let op_node = OpNode::new(args);
    let default_builder: reth_optimism_node::OpEngineApiBuilder<OpEngineValidatorBuilder> =
        OpEngineApiBuilder::default();

    let (_tx, _rx) = oneshot::channel::<Arc<dyn IntoEngineApiRpcModule + Send + Sync>>();

    // let builder_with_sender = default_builder.with_sender(tx);

    let _builder = NodeBuilder::new(config)
        .with_database(db)
        .with_types_and_provider::<OpNode, BlockchainProvider<NodeTypesWithDBAdapter<OpNode, _>>>()
        .with_components(op_node.components())
        .with_add_ons(OpAddOns::default().rpc_add_ons.with_engine_api(default_builder))
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
