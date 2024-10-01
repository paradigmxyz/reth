//! This example shows how to implement a node with a custom EVM

#![cfg_attr(not(test), warn(unused_crate_dependencies))]

use alloy_genesis::Genesis;
use alloy_primitives::{address, hex, Address, Bytes, U256};
use reth::{
    builder::{
        components::{ExecutorBuilder, PayloadServiceBuilder},
        BuilderContext, NodeBuilder,
    },
    payload::{EthBuiltPayload, EthPayloadBuilderAttributes},
    primitives::revm_primitives::{Env, PrecompileResult},
    revm::{
        handler::register::EvmHandler,
        inspector_handle_register,
        precompile::{Precompile, PrecompileOutput, PrecompileSpecId},
        primitives::BlockEnv,
        ContextPrecompiles, Database, Evm, EvmBuilder, GetInspector,
    },
    rpc::types::engine::PayloadAttributes,
    tasks::TaskManager,
    transaction_pool::TransactionPool,
};
use reth_chainspec::{Chain, ChainSpec};
use reth_evm_ethereum::EthEvmConfig;
use reth_node_api::{
    ConfigureEvm, ConfigureEvmEnv, FullNodeTypes, NextBlockEnvAttributes, NodeTypes,
    NodeTypesWithEngine, PayloadTypes,
};
use reth_node_core::{args::RpcServerArgs, node_config::NodeConfig};
use reth_node_ethereum::{
    node::{EthereumAddOns, EthereumPayloadBuilder},
    EthExecutorProvider, EthereumNode,
};
use reth_primitives::{
    revm_primitives::{CfgEnvWithHandlerCfg, PrecompileError, PrecompileErrors, TxEnv},
    Header, TransactionSigned,
};
use reth_tracing::{RethTracer, Tracer};
use serde_json::Value;
use std::sync::Arc;
use tokio::time::{sleep, Duration};

/// Custom EVM configuration
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct MyEvmConfig {
    /// Wrapper around mainnet configuration
    inner: EthEvmConfig,
}

// Maximum number of retries
const MAX_RETRIES: usize = 3;

// Fixed delay between retries (in seconds)
const RETRY_DELAY: u64 = 1;

impl MyEvmConfig {
    pub const fn new(chain_spec: Arc<ChainSpec>) -> Self {
        Self { inner: EthEvmConfig::new(chain_spec) }
    }
}

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
        let spec_id = handler.cfg.spec_id;

        handler.pre_execution.load_precompiles = Arc::new(move || {
            let mut precompiles = ContextPrecompiles::new(PrecompileSpecId::from_spec_id(spec_id));

            precompiles.extend([(
                address!("0000000000000000000000000000000000000111"),
                Precompile::Env(move |_input: &Bytes, _gas_limit: u64, _context: &Env| {
                    let handle = tokio::runtime::Handle::current();
                    let result =
                        handle.block_on(async { MyEvmConfig::state_root_precompile().await });

                    result
                })
                .into(),
            )]);

            precompiles
        });
    }

    /// Precompile execution logic for fetching the Arbitrum state root.
    /// This function is invoked by the EVM when a precompile contract at a specific address is called.
    /// It fetches the latest state root from an external service and returns it to the EVM.
    pub async fn state_root_precompile() -> PrecompileResult {
        // Call async function to fetch the state root
        match Self::fetch_state_root().await {
            Ok(state_root) => {
                // Return the state root as bytes to the EVM
                Ok(PrecompileOutput { gas_used: 0, bytes: Bytes::from(state_root.to_vec()) })
            }
            Err(e) => Err(PrecompileErrors::Error(PrecompileError::Other(format!(
                "Failed to fetch Arbitrum state root: {}",
                e
            )))),
        }
    }

    // Async function to make an HTTP request to fetch the Arbitrum state root
    async fn fetch_state_root() -> Result<[u8; 32], Box<dyn std::error::Error>> {
        let url = "http://localhost:8547";

        let request_body = r#"{
            "jsonrpc": "2.0",
            "method": "eth_getBlockByNumber",
            "params": ["latest", false],
            "id": 1
        }"#;

        let client = reqwest::Client::new();
        // Retry loop with fixed delay
        for attempt in 0..=MAX_RETRIES {
            // Send the HTTP request
            match client
                .post(url)
                .header("Content-Type", "application/json")
                .body(request_body)
                .send()
                .await
            {
                Ok(response) => {
                    let text = response
                        .text()
                        .await
                        .map_err(|e| format!("Failed to read response body: {}", e))?;

                    // Parse the JSON response
                    let json: Value = serde_json::from_str(&text)
                        .map_err(|e| format!("Failed to parse JSON response: {}", e))?;

                    // Extract the state root from the block data
                    if let Some(state_root_hex) = json["result"]["stateRoot"].as_str() {
                        let state_root_bytes = hex::decode(
                            state_root_hex.strip_prefix("0x").unwrap_or(state_root_hex),
                        )
                        .map_err(|e| format!("Failed to decode state root hex: {}", e))?;

                        if state_root_bytes.len() == 32 {
                            let mut state_root = [0u8; 32];
                            state_root.copy_from_slice(&state_root_bytes);
                            return Ok(state_root);
                        } else {
                            return Err("State root has incorrect length".into());
                        }
                    }

                    return Err("State root not found in the JSON response".into());
                }

                Err(e) => {
                    // Log the error for debugging
                    eprintln!("Attempt {} failed: {}", attempt + 1, e);

                    // If we've reached the last attempt, return the error
                    if attempt == MAX_RETRIES {
                        return Err(format!(
                            "Failed to fetch Arbitrum state root after {} attempts: {}",
                            MAX_RETRIES + 1,
                            e
                        )
                        .into());
                    }

                    // Wait for a fixed delay before retrying
                    sleep(Duration::from_secs(RETRY_DELAY)).await;
                }
            }
        }

        // This point should not be reached due to the retry mechanism.
        Err("Unexpected error in fetch_state_root".into())
    }
}

impl ConfigureEvmEnv for MyEvmConfig {
    type Header = Header;

    fn fill_tx_env(&self, tx_env: &mut TxEnv, transaction: &TransactionSigned, sender: Address) {
        self.inner.fill_tx_env(tx_env, transaction, sender);
    }

    fn fill_tx_env_system_contract_call(
        &self,
        env: &mut Env,
        caller: Address,
        contract: Address,
        data: Bytes,
    ) {
        self.inner.fill_tx_env_system_contract_call(env, caller, contract, data);
    }

    fn fill_cfg_env(
        &self,
        cfg_env: &mut CfgEnvWithHandlerCfg,
        header: &Self::Header,
        total_difficulty: U256,
    ) {
        self.inner.fill_cfg_env(cfg_env, header, total_difficulty);
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
        EvmBuilder::default()
            .with_db(db)
            // add additional precompiles
            .append_handler_register(MyEvmConfig::set_precompiles)
            .build()
    }

    fn evm_with_inspector<DB, I>(&self, db: DB, inspector: I) -> Evm<'_, I, DB>
    where
        DB: Database,
        I: GetInspector<DB>,
    {
        EvmBuilder::default()
            .with_db(db)
            .with_external_context(inspector)
            // add additional precompiles
            .append_handler_register(MyEvmConfig::set_precompiles)
            .append_handler_register(inspector_handle_register)
            .build()
    }

    fn default_external_context<'a>(&self) -> Self::DefaultExternalContext<'a> {}
}

/// Builds a regular ethereum block executor that uses the custom EVM.
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct MyExecutorBuilder;

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
        Ok((
            MyEvmConfig::new(ctx.chain_spec()),
            EthExecutorProvider::new(ctx.chain_spec(), MyEvmConfig::new(ctx.chain_spec())),
        ))
    }
}

/// Builds a regular ethereum block executor that uses the custom EVM.
#[derive(Debug, Default, Clone)]
#[non_exhaustive]
pub struct MyPayloadBuilder {
    inner: EthereumPayloadBuilder,
}

impl<Types, Node, Pool> PayloadServiceBuilder<Node, Pool> for MyPayloadBuilder
where
    Types: NodeTypesWithEngine<ChainSpec = ChainSpec>,
    Node: FullNodeTypes<Types = Types>,
    Pool: TransactionPool + Unpin + 'static,
    Types::Engine: PayloadTypes<
        BuiltPayload = EthBuiltPayload,
        PayloadAttributes = PayloadAttributes,
        PayloadBuilderAttributes = EthPayloadBuilderAttributes,
    >,
{
    async fn spawn_payload_service(
        self,
        ctx: &BuilderContext<Node>,
        pool: Pool,
    ) -> eyre::Result<reth::payload::PayloadBuilderHandle<Types::Engine>> {
        self.inner.spawn(MyEvmConfig::new(ctx.chain_spec()), ctx, pool)
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
                .payload(MyPayloadBuilder::default()),
        )
        .with_add_ons::<EthereumAddOns>()
        .launch()
        .await
        .unwrap();

    println!("Node started");

    handle.node_exit_future.await
}
