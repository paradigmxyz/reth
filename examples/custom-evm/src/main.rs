//! This example shows how to implement a node with a custom EVM

#![warn(unused_crate_dependencies)]

use alloy_genesis::Genesis;
use alloy_primitives::{address, Bytes};
use evm2::{
    evm::precompile::PrecompileOutput,
    precompiles::{Precompile, PrecompileId},
    BaseEvmTypes, SpecId,
};
use reth_ethereum::{
    chainspec::{Chain, ChainSpec},
    evm::{EthEvmConfig, EvmFactory},
    node::{
        api::{FullNodeTypes, NodeTypes},
        builder::{components::ExecutorBuilder, BuilderContext, NodeBuilder},
        core::{args::RpcServerArgs, node_config::NodeConfig},
        node::EthereumAddOns,
        EthereumNode,
    },
    tasks::Runtime,
    EthPrimitives,
};
use reth_tracing::{RethTracer, Tracer};

/// Custom EVM configuration.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct MyEvmFactory;

impl EvmFactory for MyEvmFactory {
    type Types = BaseEvmTypes;
    type SpecId = SpecId;

    fn spec_id(&self, spec: SpecId) -> SpecId {
        spec
    }

    fn tx_registry(
        &self,
        spec: SpecId,
    ) -> evm2::registry::TxRegistry<BaseEvmTypes, evm2::TxResult<BaseEvmTypes>> {
        evm2::ethereum::ethereum_tx_registry(spec)
    }

    fn precompiles(&self, spec: SpecId) -> evm2::Precompiles<BaseEvmTypes> {
        let mut precompiles = evm2::Precompiles::base(spec);
        if spec == SpecId::PRAGUE {
            precompiles.as_map_mut().insert(Precompile::new(
                address!("0x0000000000000000000000000000000000000999"),
                PrecompileId::custom("custom"),
                |_, _, _| Ok(PrecompileOutput::new(Bytes::new())),
            ));
        }
        precompiles
    }
}

/// Builds a regular ethereum block executor that uses the custom EVM.
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct MyExecutorBuilder;

impl<Node> ExecutorBuilder<Node> for MyExecutorBuilder
where
    Node: FullNodeTypes<Types: NodeTypes<ChainSpec = ChainSpec, Primitives = EthPrimitives>>,
{
    type EVM = EthEvmConfig<ChainSpec, MyEvmFactory>;

    async fn build_evm(self, ctx: &BuilderContext<Node>) -> eyre::Result<Self::EVM> {
        let evm_config =
            EthEvmConfig::new_with_evm_factory(ctx.chain_spec(), MyEvmFactory::default());
        Ok(evm_config)
    }
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let _guard = RethTracer::new().init()?;

    let runtime = Runtime::test();

    // create a custom chain spec
    let spec = ChainSpec::builder()
        .chain(Chain::mainnet())
        .genesis(Genesis::default())
        .london_activated()
        .paris_activated()
        .shanghai_activated()
        .cancun_activated()
        .prague_activated()
        .build();

    let node_config =
        NodeConfig::test().with_rpc(RpcServerArgs::default().with_http()).with_chain(spec);

    let handle = NodeBuilder::new(node_config)
        .testing_node(runtime)
        // configure the node with regular ethereum types
        .with_types::<EthereumNode>()
        // use default ethereum components but with our executor
        .with_components(EthereumNode::components().executor(MyExecutorBuilder::default()))
        .with_add_ons(EthereumAddOns::default())
        .launch()
        .await
        .unwrap();

    println!("Node started");

    handle.node_exit_future.await
}
