//! This example shows how to implement a node with a custom EVM

#![cfg_attr(not(test), warn(unused_crate_dependencies))]

use alloy_chains::Chain;
use reth::{
    builder::{node::NodeTypes, NodeBuilder},
    primitives::{
        address,
        revm_primitives::{CfgEnvWithHandlerCfg, Env, PrecompileResult, TxEnv},
        Address, Bytes, U256,
    },
    revm::{
        handler::register::EvmHandler,
        precompile::{Precompile, PrecompileSpecId, Precompiles},
        Database, Evm, EvmBuilder,
    },
    tasks::TaskManager,
};
use reth_node_api::{ConfigureEvm, ConfigureEvmEnv};
use reth_node_core::{args::RpcServerArgs, node_config::NodeConfig};
use reth_node_ethereum::{EthEngineTypes, EthEvmConfig, EthereumNode};
use reth_primitives::{ChainSpec, Genesis, Header, Transaction};
use reth_tracing::{RethTracer, Tracer};
use std::sync::Arc;

/// Custom EVM configuration
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct MyEvmConfig;

impl MyEvmConfig {
    /// Sets the precompiles to the EVM handler
    ///
    /// This will be invoked when the EVM is created via [ConfigureEvm::evm] or
    /// [ConfigureEvm::evm_with_inspector]
    ///
    /// This will use the default mainnet precompiles and add additional precompiles.
    pub fn set_precompiles<EXT, DB>(handler: &mut EvmHandler<EXT, DB>)
    where
        DB: Database,
    {
        // first we need the evm spec id, which determines the precompiles
        let spec_id = handler.cfg.spec_id;

        // install the precompiles
        handler.pre_execution.load_precompiles = Arc::new(move || {
            let mut precompiles = Precompiles::new(PrecompileSpecId::from_spec_id(spec_id)).clone();
            precompiles.inner.insert(
                address!("0000000000000000000000000000000000000999"),
                Precompile::Env(Self::my_precompile),
            );
            precompiles.into()
        });
    }

    /// A custom precompile that does nothing
    fn my_precompile(_data: &Bytes, _gas: u64, _env: &Env) -> PrecompileResult {
        Ok((0, Bytes::new()))
    }
}

impl ConfigureEvmEnv for MyEvmConfig {
    type TxMeta = ();

    fn fill_tx_env<T>(tx_env: &mut TxEnv, transaction: T, sender: Address, meta: Self::TxMeta)
    where
        T: AsRef<Transaction>,
    {
        EthEvmConfig::fill_tx_env(tx_env, transaction, sender, meta)
    }

    fn fill_cfg_env(
        cfg_env: &mut CfgEnvWithHandlerCfg,
        chain_spec: &ChainSpec,
        header: &Header,
        total_difficulty: U256,
    ) {
        EthEvmConfig::fill_cfg_env(cfg_env, chain_spec, header, total_difficulty)
    }
}

impl ConfigureEvm for MyEvmConfig {
    fn evm<'a, DB: Database + 'a>(&self, db: DB) -> Evm<'a, (), DB> {
        EvmBuilder::default()
            .with_db(db)
            // add additional precompiles
            .append_handler_register(MyEvmConfig::set_precompiles)
            .build()
    }

    fn evm_with_inspector<'a, DB: Database + 'a, I>(&self, db: DB, inspector: I) -> Evm<'a, I, DB> {
        EvmBuilder::default()
            .with_db(db)
            .with_external_context(inspector)
            // add additional precompiles
            .append_handler_register(MyEvmConfig::set_precompiles)
            .build()
    }
}

#[derive(Debug, Clone, Default)]
#[non_exhaustive]
struct MyCustomNode;

/// Configure the node types
impl NodeTypes for MyCustomNode {
    type Primitives = ();
    type Engine = EthEngineTypes;
    type Evm = MyEvmConfig;

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
        .cancun_activated()
        .build();

    let node_config =
        NodeConfig::test().with_rpc(RpcServerArgs::default().with_http()).with_chain(spec);

    let handle = NodeBuilder::new(node_config)
        .testing_node(tasks.executor())
        .with_types(MyCustomNode::default())
        .with_components(EthereumNode::components())
        .launch()
        .await
        .unwrap();

    println!("Node started");

    handle.node_exit_future.await
}
