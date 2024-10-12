//! This example shows how to implement a node with a custom EVM that uses a stateful precompile

#![cfg_attr(not(test), warn(unused_crate_dependencies))]

use alloy_genesis::Genesis;
use alloy_primitives::{Address, Bytes, U256};
use parking_lot::RwLock;
use reth::{
    api::NextBlockEnvAttributes,
    builder::{components::ExecutorBuilder, BuilderContext, NodeBuilder},
    primitives::revm_primitives::{BlockEnv, CfgEnvWithHandlerCfg, Env, PrecompileResult, TxEnv},
    revm::{
        handler::register::EvmHandler,
        inspector_handle_register,
        precompile::{Precompile, PrecompileSpecId},
        ContextPrecompile, ContextPrecompiles, Database, Evm, EvmBuilder, GetInspector,
    },
    tasks::TaskManager,
};
use reth_chainspec::{Chain, ChainSpec};
use reth_node_api::{ConfigureEvm, ConfigureEvmEnv, FullNodeTypes, NodeTypes};
use reth_node_core::{args::RpcServerArgs, node_config::NodeConfig};
use reth_node_ethereum::{node::EthereumAddOns, EthEvmConfig, EthExecutorProvider, EthereumNode};
use reth_primitives::{
    revm_primitives::{SpecId, StatefulPrecompileMut},
    Header, TransactionSigned,
};
use reth_tracing::{RethTracer, Tracer};
use schnellru::{ByLength, LruMap};
use std::{collections::HashMap, sync::Arc};

/// Type alias for the LRU cache used within the [`PrecompileCache`].
type PrecompileLRUCache = LruMap<(Bytes, u64), PrecompileResult>;

/// Type alias for the thread-safe `Arc<RwLock<_>>` wrapper around [`PrecompileCache`].
type CachedPrecompileResult = Arc<RwLock<PrecompileLRUCache>>;

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
    cache: HashMap<(Address, SpecId), CachedPrecompileResult>,
}

/// Custom EVM configuration
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct MyEvmConfig {
    inner: EthEvmConfig,
    precompile_cache: Arc<RwLock<PrecompileCache>>,
}

impl MyEvmConfig {
    /// Creates a new instance.
    pub fn new(chain_spec: Arc<ChainSpec>) -> Self {
        Self { inner: EthEvmConfig::new(chain_spec), precompile_cache: Default::default() }
    }

    /// Sets the precompiles to the EVM handler
    ///
    /// This will be invoked when the EVM is created via [ConfigureEvm::evm] or
    /// [ConfigureEvm::evm_with_inspector]
    ///
    /// This will use the default mainnet precompiles and wrap them with a cache.
    pub fn set_precompiles<EXT, DB>(
        handler: &mut EvmHandler<EXT, DB>,
        cache: Arc<RwLock<PrecompileCache>>,
    ) where
        DB: Database,
    {
        // first we need the evm spec id, which determines the precompiles
        let spec_id = handler.cfg.spec_id;

        let mut loaded_precompiles: ContextPrecompiles<DB> =
            ContextPrecompiles::new(PrecompileSpecId::from_spec_id(spec_id));
        for (address, precompile) in loaded_precompiles.to_mut().iter_mut() {
            // get or insert the cache for this address / spec
            let mut cache = cache.write();
            let cache = cache
                .cache
                .entry((*address, spec_id))
                .or_insert(Arc::new(RwLock::new(LruMap::new(ByLength::new(1024)))));

            *precompile = Self::wrap_precompile(precompile.clone(), cache.clone());
        }

        // install the precompiles
        handler.pre_execution.load_precompiles = Arc::new(move || loaded_precompiles.clone());
    }

    /// Given a [`ContextPrecompile`] and cache for a specific precompile, create a new precompile
    /// that wraps the precompile with the cache.
    fn wrap_precompile<DB>(
        precompile: ContextPrecompile<DB>,
        cache: Arc<RwLock<LruMap<(Bytes, u64), PrecompileResult>>>,
    ) -> ContextPrecompile<DB>
    where
        DB: Database,
    {
        let ContextPrecompile::Ordinary(precompile) = precompile else {
            // context stateful precompiles are not supported, due to lifetime issues or skill
            // issues
            panic!("precompile is not ordinary");
        };

        let wrapped = WrappedPrecompile { precompile, cache: cache.clone() };

        ContextPrecompile::Ordinary(Precompile::StatefulMut(Box::new(wrapped)))
    }
}

/// A custom precompile that contains the cache and precompile it wraps.
#[derive(Clone)]
pub struct WrappedPrecompile {
    /// The precompile to wrap.
    precompile: Precompile,
    /// The cache to use.
    cache: Arc<RwLock<LruMap<(Bytes, u64), PrecompileResult>>>,
}

impl StatefulPrecompileMut for WrappedPrecompile {
    fn call_mut(&mut self, bytes: &Bytes, gas_price: u64, _env: &Env) -> PrecompileResult {
        let mut cache = self.cache.write();
        let key = (bytes.clone(), gas_price);

        // get the result if it exists
        if let Some(result) = cache.get(&key) {
            return result.clone()
        }

        // call the precompile if cache miss
        let output = self.precompile.call(bytes, gas_price, _env);
        cache.insert(key, output.clone());

        output
    }
}

impl ConfigureEvmEnv for MyEvmConfig {
    type Header = Header;

    fn fill_tx_env(&self, tx_env: &mut TxEnv, transaction: &TransactionSigned, sender: Address) {
        self.inner.fill_tx_env(tx_env, transaction, sender)
    }

    fn fill_tx_env_system_contract_call(
        &self,
        env: &mut Env,
        caller: Address,
        contract: Address,
        data: Bytes,
    ) {
        self.inner.fill_tx_env_system_contract_call(env, caller, contract, data)
    }

    fn fill_cfg_env(
        &self,
        cfg_env: &mut CfgEnvWithHandlerCfg,
        header: &Self::Header,
        total_difficulty: U256,
    ) {
        self.inner.fill_cfg_env(cfg_env, header, total_difficulty)
    }

    fn next_cfg_and_block_env(
        &self,
        parent: &Self::Header,
        attributes: NextBlockEnvAttributes,
    ) -> (CfgEnvWithHandlerCfg, BlockEnv) {
        self.inner.next_cfg_and_block_env(parent, attributes)
    }
}

impl ConfigureEvm for MyEvmConfig {
    type DefaultExternalContext<'a> = ();

    fn evm<DB: Database>(&self, db: DB) -> Evm<'_, Self::DefaultExternalContext<'_>, DB> {
        let new_cache = self.precompile_cache.clone();
        EvmBuilder::default()
            .with_db(db)
            // add additional precompiles
            .append_handler_register_box(Box::new(move |handler| {
                MyEvmConfig::set_precompiles(handler, new_cache.clone())
            }))
            .build()
    }

    fn evm_with_inspector<DB, I>(&self, db: DB, inspector: I) -> Evm<'_, I, DB>
    where
        DB: Database,
        I: GetInspector<DB>,
    {
        let new_cache = self.precompile_cache.clone();
        EvmBuilder::default()
            .with_db(db)
            .with_external_context(inspector)
            // add additional precompiles
            .append_handler_register_box(Box::new(move |handler| {
                MyEvmConfig::set_precompiles(handler, new_cache.clone())
            }))
            .append_handler_register(inspector_handle_register)
            .build()
    }

    fn default_external_context<'a>(&self) -> Self::DefaultExternalContext<'a> {}
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
    Node: FullNodeTypes<Types: NodeTypes<ChainSpec = ChainSpec>>,
{
    type EVM = MyEvmConfig;
    type Executor = EthExecutorProvider<Self::EVM>;

    async fn build_evm(
        self,
        ctx: &BuilderContext<Node>,
    ) -> eyre::Result<(Self::EVM, Self::Executor)> {
        let evm_config = MyEvmConfig {
            inner: EthEvmConfig::new(ctx.chain_spec()),
            precompile_cache: self.precompile_cache.clone(),
        };
        Ok((evm_config.clone(), EthExecutorProvider::new(ctx.chain_spec(), evm_config)))
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
