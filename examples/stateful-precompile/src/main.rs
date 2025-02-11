//! This example shows how to implement a node with a custom EVM that uses a stateful precompile

#![cfg_attr(not(test), warn(unused_crate_dependencies))]

use alloy_consensus::Header;
use alloy_genesis::Genesis;
use alloy_primitives::{Address, Bytes};
use parking_lot::RwLock;
use reth::{
    api::NextBlockEnvAttributes,
    builder::{components::ExecutorBuilder, BuilderContext, NodeBuilder},
    revm::{
        handler::register::EvmHandler,
        precompile::{Precompile, PrecompileSpecId},
        primitives::{CfgEnvWithHandlerCfg, EVMError, Env, HaltReason, HandlerCfg, PrecompileResult, SpecId, StatefulPrecompileMut, TxEnv},
        ContextPrecompile, ContextPrecompiles, EvmBuilder, GetInspector,
    },
    tasks::TaskManager,
};
use reth_chainspec::{Chain, ChainSpec};
use reth_evm::{env::EvmEnv, Database};
use reth_node_api::{ConfigureEvm, ConfigureEvmEnv, FullNodeTypes, NodeTypes};
use reth_node_core::{args::RpcServerArgs, node_config::NodeConfig};
use reth_node_ethereum::{
    evm::EthEvm, node::EthereumAddOns, BasicBlockExecutorProvider, EthEvmConfig, EthExecutionStrategyFactory, EthereumNode,
};
use reth_primitives::{EthPrimitives, TransactionSigned};
use reth_tracing::{RethTracer, Tracer};
use schnellru::{ByLength, LruMap};
use std::{collections::HashMap, convert::Infallible, sync::Arc};

/// Type alias for the cache used within the PrecompileCache.
type PrecompileLRUCache = LruMap<(Bytes, u64), PrecompileResult>;
type CachedPrecompileResult = Arc<RwLock<PrecompileLRUCache>>;

/// A cache for precompile inputs/outputs.
#[derive(Debug, Default)]
pub struct PrecompileCache {
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
        Self { 
            inner: EthEvmConfig::new(chain_spec),
            precompile_cache: Default::default(),
        }
    }

    /// Sets the precompiles to the EVM handler.
    pub fn set_precompiles<EXT, DB>(handler: &mut EvmHandler<EXT, DB>, cache: Arc<RwLock<PrecompileCache>>) 
    where
        DB: Database,
    {
        let spec_id = handler.cfg.spec_id;
        let mut loaded_precompiles = ContextPrecompiles::new(PrecompileSpecId::from_spec_id(spec_id));

        for (address, precompile) in loaded_precompiles.to_mut().iter_mut() {
            let mut cache = cache.write();
            let cache = cache
                .cache
                .entry((*address, spec_id))
                .or_insert(Arc::new(RwLock::new(LruMap::new(ByLength::new(1024)))));

            *precompile = Self::wrap_precompile(precompile.clone(), cache.clone());
        }

        handler.pre_execution.load_precompiles = Arc::new(move || loaded_precompiles.clone());
    }

    fn wrap_precompile<DB>(
        precompile: ContextPrecompile<DB>,
        cache: Arc<RwLock<LruMap<(Bytes, u64), PrecompileResult>>>,
    ) -> ContextPrecompile<DB>
    where
        DB: Database,
    {
        if let ContextPrecompile::Ordinary(precompile) = precompile {
            ContextPrecompile::Ordinary(Precompile::StatefulMut(Box::new(WrappedPrecompile { precompile, cache })))
        } else {
            panic!("precompile is not ordinary");
        }
    }
}

/// A custom precompile that wraps the original precompile with a cache.
#[derive(Clone)]
pub struct WrappedPrecompile {
    precompile: Precompile,
    cache: Arc<RwLock<LruMap<(Bytes, u64), PrecompileResult>>>,
}

impl StatefulPrecompileMut for WrappedPrecompile {
    fn call_mut(&mut self, bytes: &Bytes, gas_price: u64, _env: &Env) -> PrecompileResult {
        let mut cache = self.cache.write();
        let key = (bytes.clone(), gas_price);

        if let Some(result) = cache.get(&key) {
            return result.clone();
        }

        let output = self.precompile.call(bytes, gas_price, _env);
        cache.insert(key, output.clone());

        output
    }
}

impl ConfigureEvmEnv for MyEvmConfig {
    type Header = Header;
    type Transaction = TransactionSigned;
    type Error = Infallible;
    type TxEnv = TxEnv;
    type Spec = SpecId;

    fn tx_env(&self, transaction: &Self::Transaction, signer: Address) -> Self::TxEnv {
        self.inner.tx_env(transaction, signer)
    }

    fn evm_env(&self, header: &Self::Header) -> EvmEnv {
        self.inner.evm_env(header)
    }

    fn next_evm_env(
        &self,
        parent: &Self::Header,
        attributes: NextBlockEnvAttributes,
    ) -> Result<EvmEnv, Self::Error> {
        self.inner.next_evm_env(parent, attributes)
    }
}

impl ConfigureEvm for MyEvmConfig {
    type Evm<'a, DB: Database + 'a, I: 'a> = EthEvm<'a, I, DB>;
    type EvmError<DBError: core::error::Error + Send + Sync + 'static> = EVMError<DBError>;
    type HaltReason = HaltReason;

    fn evm_with_env<DB: Database>(&self, db: DB, evm_env: EvmEnv) -> Self::Evm<'_, DB, ()> {
        let cfg_env_with_handler_cfg = CfgEnvWithHandlerCfg {
            cfg_env: evm_env.cfg_env,
            handler_cfg: HandlerCfg::new(evm_env.spec),
        };

        let new_cache = self.precompile_cache.clone();
        EvmBuilder::default()
            .with_db(db)
            .with_cfg_env_with_handler_cfg(cfg_env_with_handler_cfg)
            .with_block_env(evm_env.block_env)
            .append_handler_register_box(Box::new(move |handler| {
                MyEvmConfig::set_precompiles(handler, new_cache.clone())
            }))
            .build()
            .into()
    }

    fn evm_with_env_and_inspector<DB, I>(
        &self,
        db: DB,
        evm_env: EvmEnv,
        inspector: I,
    ) -> Self::Evm<'_, DB, I>
    where
        DB: Database,
        I: GetInspector<DB>,
    {
        let cfg_env_with_handler_cfg = CfgEnvWithHandlerCfg {
            cfg_env: evm_env.cfg_env,
            handler_cfg: HandlerCfg::new(evm_env.spec),
        };
        let new_cache = self.precompile_cache.clone();
        EvmBuilder::default()
            .with_db(db)
            .with_external_context(inspector)
            .with_cfg_env_with_handler_cfg(cfg_env_with_handler_cfg)
            .with_block_env(evm_env.block_env)
            .append_handler_register_box(Box::new(move |handler| {
                MyEvmConfig::set_precompiles(handler, new_cache.clone())
            }))
            .append_handler_register(inspector_handle_register)
            .build()
            .into()
    }
}

/// Builds a regular Ethereum block executor that uses the custom EVM.
#[derive(Debug, Default, Clone)]
#[non_exhaustive]
pub struct MyExecutorBuilder {
    precompile_cache: Arc<RwLock<PrecompileCache>>,
}

impl<Node> ExecutorBuilder<Node> for MyExecutorBuilder
where
    Node: FullNodeTypes<Types: NodeTypes<ChainSpec = ChainSpec, Primitives = EthPrimitives>>,
{
    type EVM = MyEvmConfig;
    type Executor = BasicBlockExecutorProvider<EthExecutionStrategyFactory<Self::EVM>>;

    async fn build_evm(
        self,
        ctx: &BuilderContext<Node>,
    ) -> eyre::Result<(Self::EVM, Self::Executor)> {
        let evm_config = MyEvmConfig {
            inner: EthEvmConfig::new(ctx.chain_spec()),
            precompile_cache: self.precompile_cache.clone(),
        };
        Ok((
            evm_config.clone(),
            BasicBlockExecutorProvider::new(EthExecutionStrategyFactory::new(
                ctx.chain_spec(),
                evm_config,
            )),
        ))
    }
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let _guard = RethTracer::new().init()?;

    let tasks = TaskManager::current();

    let spec = ChainSpec::builder()
        .chain(Chain::mainnet())
        .genesis(Genesis::default())
        .london_activated()
        .paris_activated()
        .shanghai_activated()
        .cancun_activated()
        .build();

    let node_config = NodeConfig::test().with_rpc(RpcServerArgs::default().with_http()).with_chain(spec);

    let handle = NodeBuilder::new(node_config)
        .testing_node(tasks.executor())
        .with_types::<EthereumNode>()
        .with_components(EthereumNode::components().executor(MyExecutorBuilder::default()))
        .with_add_ons(EthereumAddOns::default())
        .launch()
        .await
        .unwrap();

    println!("Node started");

    handle.node_exit_future.await
}

