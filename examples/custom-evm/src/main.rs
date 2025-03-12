//! This example shows how to implement a node with a custom EVM

#![warn(unused_crate_dependencies)]

use alloy_evm::{eth::EthEvmContext, EvmFactory};
use alloy_genesis::Genesis;
use alloy_primitives::{address, Address, Bytes};
use reth::{
    builder::{
        components::{BasicPayloadServiceBuilder, ExecutorBuilder, PayloadBuilderBuilder},
        BuilderContext, NodeBuilder,
    },
    payload::{EthBuiltPayload, EthPayloadBuilderAttributes},
    revm::{
        context::{Cfg, Context, TxEnv},
        context_interface::{
            result::{EVMError, HaltReason},
            ContextTr,
        },
        handler::{EthPrecompiles, PrecompileProvider},
        inspector::{Inspector, NoOpInspector},
        interpreter::{interpreter::EthInterpreter, InterpreterResult},
        precompile::{
            PrecompileError, PrecompileFn, PrecompileOutput, PrecompileResult, Precompiles,
        },
        primitives::hardfork::SpecId,
        MainBuilder, MainContext,
    },
    rpc::types::engine::PayloadAttributes,
    tasks::TaskManager,
    transaction_pool::{PoolTransaction, TransactionPool},
};
use reth_chainspec::{Chain, ChainSpec};
use reth_evm::{Database, EvmEnv};
use reth_evm_ethereum::{EthEvm, EthEvmConfig};
use reth_node_api::{FullNodeTypes, NodeTypes, NodeTypesWithEngine, PayloadTypes};
use reth_node_core::{args::RpcServerArgs, node_config::NodeConfig};
use reth_node_ethereum::{
    node::{EthereumAddOns, EthereumPayloadBuilder},
    BasicBlockExecutorProvider, EthereumNode,
};
use reth_primitives::{EthPrimitives, TransactionSigned};
use reth_tracing::{RethTracer, Tracer};
use std::sync::OnceLock;

/// Custom EVM configuration.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct MyEvmFactory;

impl EvmFactory for MyEvmFactory {
    type Evm<DB: Database, I: Inspector<EthEvmContext<DB>, EthInterpreter>> =
        EthEvm<DB, I, CustomPrecompiles>;
    type Tx = TxEnv;
    type Error<DBError: core::error::Error + Send + Sync + 'static> = EVMError<DBError>;
    type HaltReason = HaltReason;
    type Context<DB: Database> = EthEvmContext<DB>;
    type Spec = SpecId;

    fn create_evm<DB: Database>(&self, db: DB, input: EvmEnv) -> Self::Evm<DB, NoOpInspector> {
        let evm = Context::mainnet()
            .with_db(db)
            .with_cfg(input.cfg_env)
            .with_block(input.block_env)
            .build_mainnet_with_inspector(NoOpInspector {})
            .with_precompiles(CustomPrecompiles::new());

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
    type Executor = BasicBlockExecutorProvider<Self::EVM>;

    async fn build_evm(
        self,
        ctx: &BuilderContext<Node>,
    ) -> eyre::Result<(Self::EVM, Self::Executor)> {
        let evm_config =
            EthEvmConfig::new_with_evm_factory(ctx.chain_spec(), MyEvmFactory::default());
        Ok((evm_config.clone(), BasicBlockExecutorProvider::new(evm_config)))
    }
}

/// Builds a regular ethereum block executor that uses the custom EVM.
#[derive(Debug, Default, Clone)]
#[non_exhaustive]
pub struct MyPayloadBuilder {
    inner: EthereumPayloadBuilder,
}

impl<Types, Node, Pool> PayloadBuilderBuilder<Node, Pool> for MyPayloadBuilder
where
    Types: NodeTypesWithEngine<ChainSpec = ChainSpec, Primitives = EthPrimitives>,
    Node: FullNodeTypes<Types = Types>,
    Pool: TransactionPool<Transaction: PoolTransaction<Consensus = TransactionSigned>>
        + Unpin
        + 'static,
    Types::Engine: PayloadTypes<
        BuiltPayload = EthBuiltPayload,
        PayloadAttributes = PayloadAttributes,
        PayloadBuilderAttributes = EthPayloadBuilderAttributes,
    >,
{
    type PayloadBuilder = reth_ethereum_payload_builder::EthereumPayloadBuilder<
        Pool,
        Node::Provider,
        EthEvmConfig<MyEvmFactory>,
    >;

    async fn build_payload_builder(
        self,
        ctx: &BuilderContext<Node>,
        pool: Pool,
    ) -> eyre::Result<Self::PayloadBuilder> {
        let evm_config =
            EthEvmConfig::new_with_evm_factory(ctx.chain_spec(), MyEvmFactory::default());
        self.inner.build(evm_config, ctx, pool)
    }
}

/// A custom precompile that contains static precompiles.
#[derive(Clone)]
pub struct CustomPrecompiles {
    pub precompiles: EthPrecompiles,
}

impl CustomPrecompiles {
    /// Given a [`PrecompileProvider`] and cache for a specific precompiles, create a
    /// wrapper that can be used inside Evm.
    fn new() -> Self {
        Self { precompiles: EthPrecompiles::default() }
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

impl<CTX: ContextTr> PrecompileProvider<CTX> for CustomPrecompiles {
    type Output = InterpreterResult;

    fn set_spec(&mut self, spec: <CTX::Cfg as Cfg>::Spec) {
        let spec_id = spec.clone().into();
        if spec_id == SpecId::PRAGUE {
            self.precompiles = EthPrecompiles { precompiles: prague_custom() }
        } else {
            PrecompileProvider::<CTX>::set_spec(&mut self.precompiles, spec);
        }
    }

    fn run(
        &mut self,
        context: &mut CTX,
        address: &Address,
        bytes: &Bytes,
        gas_limit: u64,
    ) -> Result<Option<Self::Output>, PrecompileError> {
        self.precompiles.run(context, address, bytes, gas_limit)
    }

    fn contains(&self, address: &Address) -> bool {
        self.precompiles.contains(address)
    }

    fn warm_addresses(&self) -> Box<impl Iterator<Item = Address>> {
        self.precompiles.warm_addresses()
    }
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
        .build();

    let node_config =
        NodeConfig::test().with_rpc(RpcServerArgs::default().with_http()).with_chain(spec);

    let handle = NodeBuilder::new(node_config)
        .testing_node(tasks.executor())
        // configure the node with regular ethereum types
        .with_types::<EthereumNode>()
        // use default ethereum components but with our executor
        .with_components(
            EthereumNode::components()
                .executor(MyExecutorBuilder::default())
                .payload(BasicPayloadServiceBuilder::new(MyPayloadBuilder::default())),
        )
        .with_add_ons(EthereumAddOns::default())
        .launch()
        .await
        .unwrap();

    println!("Node started");

    handle.node_exit_future.await
}
