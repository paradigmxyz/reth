//! This example shows how to implement a node with a custom EVM

#![warn(unused_crate_dependencies)]

use alloy_evm::{eth::EthEvmContext, precompiles::PrecompilesMap, EvmFactory};
use alloy_genesis::Genesis;
use alloy_primitives::{address, Bytes};
use reth_ethereum::{
    chainspec::{Chain, ChainSpec},
    evm::{
        primitives::{Database, EvmEnv},
        revm::{
            context::{Context, TxEnv},
            context_interface::result::{EVMError, HaltReason},
            handler::EthPrecompiles,
            inspector::{Inspector, NoOpInspector},
            interpreter::interpreter::EthInterpreter,
            precompile::{PrecompileFn, PrecompileOutput, PrecompileResult, Precompiles},
            primitives::hardfork::SpecId,
            MainBuilder, MainContext,
        },
        EthEvm, EthEvmConfig,
    },
    node::{
        api::{FullNodeTypes, NodeTypes},
        builder::{components::ExecutorBuilder, BuilderContext, NodeBuilder},
        core::{args::RpcServerArgs, node_config::NodeConfig},
        node::EthereumAddOns,
        EthereumNode,
    },
    tasks::TaskManager,
    EthPrimitives,
};
use reth_tracing::{RethTracer, Tracer};
use std::sync::OnceLock;

/// Custom EVM configuration.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct MyEvmFactory;

impl EvmFactory for MyEvmFactory {
    type Evm<DB: Database, I: Inspector<EthEvmContext<DB>, EthInterpreter>> =
        EthEvm<DB, I, Self::Precompiles>;
    type Tx = TxEnv;
    type Error<DBError: core::error::Error + Send + Sync + 'static> = EVMError<DBError>;
    type HaltReason = HaltReason;
    type Context<DB: Database> = EthEvmContext<DB>;
    type Spec = SpecId;
    type Precompiles = PrecompilesMap;

    fn create_evm<DB: Database>(&self, db: DB, input: EvmEnv) -> Self::Evm<DB, NoOpInspector> {
        let spec = input.cfg_env.spec;
        let mut evm = Context::mainnet()
            .with_db(db)
            .with_cfg(input.cfg_env)
            .with_block(input.block_env)
            .build_mainnet_with_inspector(NoOpInspector {})
            .with_precompiles(PrecompilesMap::from_static(EthPrecompiles::default().precompiles));

        if spec == SpecId::PRAGUE {
            evm = evm.with_precompiles(PrecompilesMap::from_static(prague_custom()));
        }

        EthEvm::new(evm, false)
    }

    fn create_evm_with_inspector<DB: Database, I: Inspector<Self::Context<DB>, EthInterpreter>>(
        &self,
        db: DB,
        input: EvmEnv,
        inspector: I,
    ) -> Self::Evm<DB, I> {
        EthEvm::new(self.create_evm(db, input).into_inner().with_inspector(inspector), true)
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
    type EVM = EthEvmConfig<MyEvmFactory>;

    async fn build_evm(self, ctx: &BuilderContext<Node>) -> eyre::Result<Self::EVM> {
        let evm_config =
            EthEvmConfig::new_with_evm_factory(ctx.chain_spec(), MyEvmFactory::default());
        Ok(evm_config)
    }
}

/// Returns precompiles for Fjor spec.
pub fn prague_custom() -> &'static Precompiles {
    static INSTANCE: OnceLock<Precompiles> = OnceLock::new();
    INSTANCE.get_or_init(|| {
        let mut precompiles = Precompiles::prague().clone();
        // Custom precompile.
        precompiles.extend([(
            address!("0x0000000000000000000000000000000000000999"),
            |_, _| -> PrecompileResult {
                PrecompileResult::Ok(PrecompileOutput::new(0, Bytes::new()))
            } as PrecompileFn,
        )
            .into()]);
        precompiles
    })
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let _guard = RethTracer::new().init()?;

    let tasks = TaskManager::current();

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
        .testing_node(tasks.executor())
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
