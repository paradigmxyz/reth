//! This example shows how to implement a node with a custom EVM

#![warn(unused_crate_dependencies)]

use alloy_chains::Chain;

use reth::{
    builder::{node::NodeTypes, Node},
    tasks::TaskManager,
};
use reth_node_api::{EngineTypes, PayloadBuilderAttributes};
use reth_node_ethereum::{EthEngineTypes, EthEvmConfig};
use reth_primitives::{ChainSpec, Genesis};
use reth_tracing::{RethTracer, Tracer};

#[derive(Debug, Clone, Default)]
#[non_exhaustive]
struct MyCustomNode;

/// Configure the node types
impl NodeTypes for MyCustomNode {
    type Primitives = ();
    type Engine = EthEngineTypes;
    type Evm = EthEvmConfig;

    fn evm_config(&self) -> Self::Evm {
        Self::Evm::default()
    }
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let _guard = RethTracer::new().init()?;

    let tasks = TaskManager::current();

    // create optimism genesis with canyon at block 2
    let spec = ChainSpec::builder()
        .chain(Chain::mainnet())
        .genesis(Genesis::default())
        .london_activated()
        .paris_activated()
        .shanghai_activated()
        .build();

    // create node config
    // let node_config =
    //     NodeConfig::test().with_rpc(RpcServerArgs::default().with_http()).with_chain(spec);
    //
    // let handle = NodeBuilder::new(node_config)
    //     .testing_node(tasks.executor())
    //     .launch_node(MyCustomNode::default())
    //     .await
    //     .unwrap();
    //
    // println!("Node started");
    //
    // handle.node_exit_future.await

    Ok(())
}
