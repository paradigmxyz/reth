//! This example shows how to implement a node with a custom EVM that uses a stateful precompile

#![warn(unused_crate_dependencies)]

use alloy_evm::{eth::EthEvmContext, EvmFactory};
use alloy_genesis::Genesis;
use alloy_primitives::{Address, Bytes};
use parking_lot::RwLock;
use reth::{
    builder::{components::ExecutorBuilder, BuilderContext, NodeBuilder},
    revm::{
        context::{Cfg, Context, TxEnv},
        context_interface::{
            result::{EVMError, HaltReason},
            ContextTr,
        },
        handler::{EthPrecompiles, PrecompileProvider},
        inspector::{Inspector, NoOpInspector},
        interpreter::{interpreter::EthInterpreter, InterpreterResult},
        precompile::PrecompileError,
        primitives::hardfork::SpecId,
        MainBuilder, MainContext,
    },
    tasks::TaskManager,
};
use reth_chainspec::{Chain, ChainSpec};
use reth_evm::{Database, EvmEnv};
use reth_node_api::{FullNodeTypes, NodeTypes};
use reth_node_core::{args::RpcServerArgs, node_config::NodeConfig};
use reth_node_ethereum::{
    evm::EthEvm, node::EthereumAddOns, BasicBlockExecutorProvider, EthEvmConfig, EthereumNode,
};
use reth_primitives::EthPrimitives;
use reth_tracing::{RethTracer, Tracer};
use schnellru::{ByLength, LruMap};
use std::{collections::HashMap, sync::Arc};

/// Type alias for the LRU cache used within the [`PrecompileCache`].
type PrecompileLRUCache = LruMap<(SpecId, Bytes, u64), Result<InterpreterResult, PrecompileError>>;

type WrappedEthEvm<DB, I> = EthEvm<DB, I, WrappedPrecompile<EthPrecompiles>>;

/// A cache for precompile inputs / outputs.
///
/// This assumes that the precompile is a standard precompile, as in `StandardPrecompileFn`, meaning
/// its inputs are only `(Bytes, u64)`.
///
/// NOTE: This does not work with "context stateful precompiles", ie `ContextStatefulPrecompile` or
/// `ContextStatefulPrecompileMut`. They are explicitly banned.
#[derive(Debug, Default)]
pub struct PrecompileCache {
    /// Caches for each precompile input / output.
    cache: HashMap<Address, PrecompileLRUCache>,
}

/// Custom EVM factory.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct MyEvmFactory {
    precompile_cache: Arc<RwLock<PrecompileCache>>,
}

impl EvmFactory for MyEvmFactory {
    type Evm<DB: Database, I: Inspector<EthEvmContext<DB>, EthInterpreter>> = WrappedEthEvm<DB, I>;
    type Tx = TxEnv;
    type Error<DBError: core::error::Error + Send + Sync + 'static> = EVMError<DBError>;
    type HaltReason = HaltReason;
    type Context<DB: Database> = EthEvmContext<DB>;
    type Spec = SpecId;

    fn create_evm<DB: Database>(&self, db: DB, input: EvmEnv) -> Self::Evm<DB, NoOpInspector> {
        let new_cache = self.precompile_cache.clone();

        let evm = Context::mainnet()
            .with_db(db)
            .with_cfg(input.cfg_env)
            .with_block(input.block_env)
            .build_mainnet_with_inspector(NoOpInspector {})
            .with_precompiles(WrappedPrecompile::new(EthPrecompiles::default(), new_cache));

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

/// A custom precompile that contains the cache and precompile it wraps.
#[derive(Clone)]
pub struct WrappedPrecompile<P> {
    /// The precompile to wrap.
    precompile: P,
    /// The cache to use.
    cache: Arc<RwLock<PrecompileCache>>,
    /// The spec id to use.
    spec: SpecId,
}

impl<P> WrappedPrecompile<P> {
    /// Given a [`PrecompileProvider`] and cache for a specific precompiles, create a
    /// wrapper that can be used inside Evm.
    fn new(precompile: P, cache: Arc<RwLock<PrecompileCache>>) -> Self {
        WrappedPrecompile { precompile, cache: cache.clone(), spec: SpecId::LATEST }
    }
}

impl<CTX: ContextTr, P: PrecompileProvider<CTX, Output = InterpreterResult>> PrecompileProvider<CTX>
    for WrappedPrecompile<P>
{
    type Output = P::Output;

    fn set_spec(&mut self, spec: <CTX::Cfg as Cfg>::Spec) {
        self.precompile.set_spec(spec.clone());
        self.spec = spec.into();
    }

    fn run(
        &mut self,
        context: &mut CTX,
        address: &Address,
        bytes: &Bytes,
        gas_limit: u64,
    ) -> Result<Option<Self::Output>, PrecompileError> {
        let mut cache = self.cache.write();
        let key = (self.spec, bytes.clone(), gas_limit);

        // get the result if it exists
        if let Some(precompiles) = cache.cache.get_mut(address) {
            if let Some(result) = precompiles.get(&key) {
                return result.clone().map(Some)
            }
        }

        // call the precompile if cache miss
        let output = self.precompile.run(context, address, bytes, gas_limit);

        if let Some(output) = output.clone().transpose() {
            // insert the result into the cache
            cache
                .cache
                .entry(*address)
                .or_insert(PrecompileLRUCache::new(ByLength::new(1024)))
                .insert(key, output);
        }

        output
    }

    fn contains(&self, address: &Address) -> bool {
        self.precompile.contains(address)
    }

    fn warm_addresses(&self) -> Box<impl Iterator<Item = Address>> {
        self.precompile.warm_addresses()
    }
}

/// Builds a regular ethereum block executor that uses the custom EVM.
#[derive(Debug, Default, Clone)]
#[non_exhaustive]
pub struct MyExecutorBuilder {
    /// The precompile cache to use for all executors.
    precompile_cache: Arc<RwLock<PrecompileCache>>,
}

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
        let evm_config = EthEvmConfig::new_with_evm_factory(
            ctx.chain_spec(),
            MyEvmFactory { precompile_cache: self.precompile_cache.clone() },
        );
        Ok((evm_config.clone(), BasicBlockExecutorProvider::new(evm_config)))
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
        .with_components(EthereumNode::components().executor(MyExecutorBuilder::default()))
        .with_add_ons(EthereumAddOns::default())
        .launch()
        .await
        .unwrap();

    println!("Node started");

    handle.node_exit_future.await
}
