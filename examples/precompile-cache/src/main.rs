//! This example shows how to implement a node with a custom EVM that uses a stateful precompile

#![warn(unused_crate_dependencies)]

use alloy_evm::{
    eth::EthEvmContext,
    precompiles::{DynPrecompile, Precompile, PrecompilesMap},
    Evm, EvmFactory,
};
use alloy_genesis::Genesis;
use alloy_primitives::Bytes;
use parking_lot::RwLock;
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
            precompile::PrecompileResult,
            primitives::hardfork::SpecId,
            MainBuilder, MainContext,
        },
    },
    node::{
        api::{FullNodeTypes, NodeTypes},
        builder::{components::ExecutorBuilder, BuilderContext, NodeBuilder},
        core::{args::RpcServerArgs, node_config::NodeConfig},
        evm::EthEvm,
        node::EthereumAddOns,
        EthEvmConfig, EthereumNode,
    },
    tasks::TaskManager,
    EthPrimitives,
};
use reth_tracing::{RethTracer, Tracer};
use schnellru::{ByLength, LruMap};
use std::sync::Arc;

/// Type alias for the LRU cache used within the [`PrecompileCache`].
type PrecompileLRUCache = LruMap<(Bytes, u64), PrecompileResult>;

/// A cache for precompile inputs / outputs.
///
/// This assumes that the precompile is a standard precompile, as in `StandardPrecompileFn`, meaning
/// its inputs are only `(Bytes, u64)`.
///
/// NOTE: This does not work with "context stateful precompiles", ie `ContextStatefulPrecompile` or
/// `ContextStatefulPrecompileMut`. They are explicitly banned.
#[derive(Debug)]
pub struct PrecompileCache {
    /// Caches for each precompile input / output.
    cache: PrecompileLRUCache,
}

/// Custom EVM factory.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct MyEvmFactory {
    precompile_cache: Arc<RwLock<PrecompileCache>>,
}

impl EvmFactory for MyEvmFactory {
    type Evm<DB: Database, I: Inspector<EthEvmContext<DB>, EthInterpreter>> =
        EthEvm<DB, I, PrecompilesMap>;
    type Tx = TxEnv;
    type Error<DBError: core::error::Error + Send + Sync + 'static> = EVMError<DBError>;
    type HaltReason = HaltReason;
    type Context<DB: Database> = EthEvmContext<DB>;
    type Spec = SpecId;
    type Precompiles = PrecompilesMap;

    fn create_evm<DB: Database>(&self, db: DB, input: EvmEnv) -> Self::Evm<DB, NoOpInspector> {
        let new_cache = self.precompile_cache.clone();

        let evm = Context::mainnet()
            .with_db(db)
            .with_cfg(input.cfg_env)
            .with_block(input.block_env)
            .build_mainnet_with_inspector(NoOpInspector {})
            .with_precompiles(PrecompilesMap::from_static(EthPrecompiles::default().precompiles));

        let mut evm = EthEvm::new(evm, false);

        evm.precompiles_mut().map_precompiles(|_, precompile| {
            WrappedPrecompile::wrap(precompile, new_cache.clone())
        });

        evm
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
pub struct WrappedPrecompile {
    /// The precompile to wrap.
    precompile: DynPrecompile,
    /// The cache to use.
    cache: Arc<RwLock<PrecompileCache>>,
}

impl WrappedPrecompile {
    fn new(precompile: DynPrecompile, cache: Arc<RwLock<PrecompileCache>>) -> Self {
        Self { precompile, cache }
    }

    /// Given a [`DynPrecompile`] and cache for a specific precompiles, create a
    /// wrapper that can be used inside Evm.
    fn wrap(precompile: DynPrecompile, cache: Arc<RwLock<PrecompileCache>>) -> DynPrecompile {
        let wrapped = Self::new(precompile, cache);
        move |data: &[u8], gas_limit: u64| -> PrecompileResult { wrapped.call(data, gas_limit) }
            .into()
    }
}

impl Precompile for WrappedPrecompile {
    fn call(&self, data: &[u8], gas: u64) -> PrecompileResult {
        let mut cache = self.cache.write();
        let key = (Bytes::copy_from_slice(data), gas);

        // get the result if it exists
        if let Some(result) = cache.cache.get(&key) {
            return result.clone()
        }

        // call the precompile if cache miss
        let output = self.precompile.call(data, gas);

        // insert the result into the cache
        cache.cache.insert(key, output.clone());

        output
    }
}

/// Builds a regular ethereum block executor that uses the custom EVM.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct MyExecutorBuilder {
    /// The precompile cache to use for all executors.
    precompile_cache: Arc<RwLock<PrecompileCache>>,
}

impl Default for MyExecutorBuilder {
    fn default() -> Self {
        let precompile_cache = PrecompileCache {
            cache: LruMap::<(Bytes, u64), PrecompileResult>::new(ByLength::new(100)),
        };
        Self { precompile_cache: Arc::new(RwLock::new(precompile_cache)) }
    }
}

impl<Node> ExecutorBuilder<Node> for MyExecutorBuilder
where
    Node: FullNodeTypes<Types: NodeTypes<ChainSpec = ChainSpec, Primitives = EthPrimitives>>,
{
    type EVM = EthEvmConfig<MyEvmFactory>;

    async fn build_evm(self, ctx: &BuilderContext<Node>) -> eyre::Result<Self::EVM> {
        let evm_config = EthEvmConfig::new_with_evm_factory(
            ctx.chain_spec(),
            MyEvmFactory { precompile_cache: self.precompile_cache.clone() },
        );
        Ok(evm_config)
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
