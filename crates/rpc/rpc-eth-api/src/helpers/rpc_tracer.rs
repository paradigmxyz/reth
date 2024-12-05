use alloy_consensus::{Header, Transaction};
use reth_chainspec::{ChainSpec, EthChainSpec};
use reth_evm::{system_calls::SystemCaller, ConfigureEvm};
use reth_primitives::{SealedBlockWithSenders, TransactionSigned};
use reth_provider::{BlockIdReader, ChainSpecProvider, StateProvider, StateProviderFactory};
use reth_revm::database::StateProviderDatabase;
use revm::{db::CacheDB, Database, DatabaseCommit, DatabaseRef, Evm, EvmBuilder, GetInspector};
use revm_inspectors::tracing::{TracingInspector, TracingInspectorConfig};
use revm_primitives::{
    Address, BlockEnv, CfgEnv, CfgEnvWithHandlerCfg, EnvWithHandlerCfg, HandlerCfg, ResultAndState,
    TxEnv, B256, U256,
};
use std::{
    error::Error as StdError,
    fmt::{self, Debug, Display},
    future::Future,
    sync::Arc,
};

use super::{Call, LoadTransaction, Trace};

/// Trait that combines all required provider capabilities
pub trait ProviderCapabilities:
    StateProvider + BlockIdReader + ChainSpecProvider<ChainSpec = ChainSpec> + StateProviderFactory
{
}

/// Handles tracing and execution of transactions with system calls integration
#[derive(Debug, Clone)]
pub struct RpcTracer<P>
where
    P: ?Sized + ProviderCapabilities,
{
    provider: Box<P>,
    config: Arc<TracingConfig>,
}

/// Configuration for the RPC tracer
#[derive(Clone, Debug)]
pub struct TracingConfig {
    /// Gas limit for call operations
    pub call_gas_limit: u64,
    /// Maximum number of blocks that can be simulated
    pub max_simulate_blocks: u64,
}

impl<P> RpcTracer<P>
where
    P: ?Sized + ProviderCapabilities,
{
    /// Create a new RPC tracer instance with default config
    pub fn new(provider: P) -> Self
    where
        P: Sized,
    {
        Self {
            provider: Box::new(provider),
            config: Arc::new(TracingConfig {
                call_gas_limit: 50_000_000,
                max_simulate_blocks: 100,
            }),
        }
    }

    /// Create a new RPC tracer instance with custom config
    pub fn with_config(provider: P, config: TracingConfig) -> Self
    where
        P: Sized,
    {
        Self { provider: Box::new(provider), config: Arc::new(config) }
    }

    /// Execute a transaction trace with system calls
    pub fn trace_transaction_with_system_calls<'a, DB, I>(
        &self,
        db: &'a mut DB,
        env: EnvWithHandlerCfg,
        inspector: I,
        pre_execution_hook: impl FnOnce(
            &mut DB,
            &EnvWithHandlerCfg,
            &BlockEnv,
        ) -> Result<(), Box<dyn StdError>>,
    ) -> Result<(ResultAndState, EnvWithHandlerCfg), Box<dyn StdError>>
    where
        DB: Database + DatabaseCommit + 'a,
        <DB as Database>::Error: StdError + 'static,
        I: GetInspector<&'a mut DB> + Debug,
    {
        pre_execution_hook(db, &env, &env.block)?;
        let mut evm = self.build_evm_with_env_and_inspector(db, env, inspector);
        let res = evm.transact().map_err(|e| Box::new(e) as Box<dyn StdError>)?;
        let (_, env) = evm.into_db_and_env_with_handler_cfg();
        Ok((res, env))
    }

    /// Replay transactions in a block until a specific transaction
    pub fn replay_block_transactions<'a, DB, I>(
        &self,
        db: &'a mut DB,
        cfg: CfgEnvWithHandlerCfg,
        block_env: BlockEnv,
        block: &SealedBlockWithSenders,
        target_tx_hash: B256,
        inspector: I,
    ) -> Result<usize, Box<dyn StdError>>
    where
        DB: Database + DatabaseCommit,
        <DB as Database>::Error: StdError + 'static,
        I: GetInspector<&'a mut DB>,
    {
        let mut index = 0;
        let env = EnvWithHandlerCfg::new_with_cfg_env(cfg, block_env, Default::default());
        let mut evm: Evm<'_, _, &mut DB> =
            self.build_evm_with_env_and_inspector(db, env, inspector);

        for (sender, tx) in block.transactions_with_sender() {
            if tx.hash() == target_tx_hash {
                break
            }

            let tx_env = self.build_tx_env(tx, *sender);
            evm.context.evm.env.tx = tx_env;
            evm.transact_commit().map_err(|e| Box::new(e) as Box<dyn StdError>)?;
            index += 1;
        }

        Ok(index)
    }

    /// Trace a full block with system calls
    pub async fn trace_block<R>(
        &self,
        block: Arc<SealedBlockWithSenders>,
        config: TracingInspectorConfig,
        mut callback: impl FnMut(TracingInspector, ResultAndState) -> Result<R, Box<dyn StdError>>,
    ) -> Result<Vec<R>, Box<dyn StdError>>
    where
        R: Send + 'static,
    {
        let mut results = Vec::with_capacity(block.body.transactions.len());

        let state = self
            .provider
            .state_by_block_hash(block.parent_hash)
            .map_err(|e| Box::new(e) as Box<dyn StdError>)?;
        let state_db = StateProviderDatabase::new(state);
        let mut cache_db = CacheDB::new(state_db);

        self.apply_pre_block_system_calls(&mut cache_db, &block)?;

        for (sender, tx) in block.transactions_with_sender() {
            let mut inspector = TracingInspector::new(config.clone());
            let env = self.prepare_transaction_env(tx, *sender, &block)?;

            let (res, _) = self.trace_transaction_with_system_calls(
                &mut cache_db,
                env,
                &mut inspector,
                |db, cfg, block_env| self.apply_system_calls(db, cfg, block_env, &block),
            )?;

            results.push(callback(inspector, res.clone())?);
            cache_db.commit(res.state);
        }

        Ok(results)
    }

    /// Prepares the transaction environment for execution
    pub fn prepare_transaction_env(
        &self,
        tx: &TransactionSigned,
        sender: Address,
        block: &SealedBlockWithSenders,
    ) -> Result<EnvWithHandlerCfg, Box<dyn StdError>> {
        let block_env = BlockEnv {
            number: U256::from(block.number),
            coinbase: block.header.beneficiary,
            timestamp: U256::from(block.timestamp),
            difficulty: block.header.difficulty,
            prevrandao: Some(block.header.mix_hash),
            basefee: U256::from(block.header.base_fee_per_gas.unwrap_or_default()),
            gas_limit: U256::from(block.header.gas_limit),
            ..Default::default()
        };

        let cfg =
            CfgEnvWithHandlerCfg { cfg_env: CfgEnv::default(), handler_cfg: HandlerCfg::default() };

        let tx_env = self.build_tx_env(tx, sender);

        Ok(EnvWithHandlerCfg::new_with_cfg_env(cfg, block_env, tx_env))
    }

    /// Build EVM instance with environment and inspector
    pub fn build_evm_with_env_and_inspector<'a, DB, I>(
        &self,
        db: DB,
        env: EnvWithHandlerCfg,
        inspector: I,
    ) -> revm::Evm<'a, I, DB>
    where
        I: GetInspector<DB>,
        DB: revm::Database,
    {
        EvmBuilder::default()
            .with_db(db)
            .with_spec_id(env.clone().spec_id())
            .with_env(env.env)
            .with_external_context(inspector)
            .build()
    }

    /// Build transaction environment
    pub fn build_tx_env(&self, tx: &TransactionSigned, sender: Address) -> TxEnv {
        TxEnv {
            caller: sender,
            gas_price: U256::from(tx.effective_gas_price(None)),
            gas_priority_fee: Some(U256::from(tx.max_priority_fee_per_gas().unwrap_or_default())),
            transact_to: revm_primitives::TxKind::Call(tx.to().unwrap_or_default()),
            data: tx.input().clone(),
            chain_id: tx.chain_id(),
            access_list: tx.access_list().unwrap().iter().cloned().collect(),
            value: U256::from(tx.value()),
            gas_limit: tx.gas_limit(),
            ..Default::default()
        }
    }

    /// Apply pre-block system calls
    pub fn apply_pre_block_system_calls<DB>(
        &self,
        db: &mut DB,
        block: &SealedBlockWithSenders,
    ) -> Result<(), Box<dyn StdError>>
    where
        DB: Database + DatabaseCommit,
        <DB as Database>::Error: StdError + 'static,
    {
        // Create the necessary environments for system calls
        let block_env = BlockEnv {
            number: U256::from(block.number),
            timestamp: U256::from(block.timestamp),
            difficulty: block.difficulty,
            gas_limit: U256::from(block.gas_limit),
            basefee: U256::from(block.base_fee_per_gas.unwrap()),
            ..Default::default()
        };

        let cfg = CfgEnv::default();
        //find a proper way to get the specid
        let handler = HandlerCfg::default();
        let cfg = CfgEnvWithHandlerCfg {
            cfg_env: CfgEnv::with_chain_id(cfg, self.provider.chain_spec().chain_id()),
            handler_cfg: handler,
        };
        // Create system caller
        let mut system_caller: SystemCaller<_, Arc<ChainSpec>> =
            SystemCaller::new(cfg.clone(), self.provider.chain_spec());
        
        

        // // Apply EIP-4788 beacon root system call
        // system_caller
        //     .pre_block_beacon_root_contract_call(
        //         db,
        //         &cfg,
        //         &block_env,
        //         block.header.parent_beacon_block_root,
        //     )
        //     .map_err(|e| Box::new(e) as Box<dyn StdError>)?;
        Ok(())
    }

    /// Apply system calls before transaction execution
    pub fn apply_system_calls<DB>(
        &self,
        _db: &mut DB,
        _cfg: &EnvWithHandlerCfg,
        _block_env: &BlockEnv,
        _block: &SealedBlockWithSenders,
    ) -> Result<(), Box<dyn StdError>>
    where
        DB: Database + DatabaseCommit,
        <DB as Database>::Error: StdError + 'static,
    {
        // // Create system caller with the chain spec's EVM config
        // let mut system_caller = SystemCaller::<_, Arc<ChainSpec>>::new(
        //     config,
        //     chain_spec.clone(),
        // );

        // Apply pre-transaction system calls
        todo!()
    }
}

/// Extension trait to integrate RpcTracer with existing Call and Trace traits
pub trait RpcTracerExt<T>: Call + Trace + LoadTransaction
where
    T: BlockIdReader
        + StateProvider
        + ChainSpecProvider<ChainSpec = ChainSpec>
        + StateProviderFactory,
{
    /// Returns a handle for reading evm config.
    ///
    /// Data access in default (L1) trait method implementations.
    fn evm_config(&self) -> &impl ConfigureEvm<Header = Header>;

    /// Error type for tracer operations
    type Error: StdError + Send + Sync + 'static;

    /// Get the RPC tracer instance
    fn tracer(&self) -> &RpcTracer<dyn ProviderCapabilities>;

    /// Convert from standard error
    fn from_std_error(err: Box<dyn StdError>) -> <Self as RpcTracerExt<T>>::Error;

    /// Get state provider by block hash
    fn state_by_block_hash(
        &self,
        block_hash: B256,
    ) -> Result<impl StateProvider + DatabaseRef, <Self as RpcTracerExt<T>>::Error>;

    /// Trace a transaction with full system call integration
    fn trace_transaction(
        &self,
        hash: B256,
        config: TracingInspectorConfig,
    ) -> impl Future<Output = Result<Option<ResultAndState>, <Self as RpcTracerExt<T>>::Error>> + Send
    {
        async move {
            let (tx, block) = match self.transaction_and_block(hash).await.unwrap() {
                Some(res) => res,
                None => return Ok(None),
            };

            let tracer = self.tracer();

            let state = self.state_by_block_hash(block.parent_hash)?;
            let state_db = StateProviderDatabase::new(state);
            let mut cache_db = CacheDB::new(state_db);

            let env = tracer
                .prepare_transaction_env(
                    &tx.clone().into_recovered().as_signed(),
                    tx.into_recovered().signer(),
                    &block,
                )
                .map_err(<Self as RpcTracerExt<T>>::from_std_error)?;

            let res = tracer
                .trace_transaction_with_system_calls(
                    &mut cache_db,
                    env,
                    TracingInspector::new(config),
                    |db, cfg, block_env| tracer.apply_system_calls(db, cfg, block_env, &block),
                )
                .map_err(<Self as RpcTracerExt<T>>::from_std_error)?;

            Ok(Some(res.0))
        }
    }
}



/// Error type for RPC tracer operations
#[derive(Debug)]
pub enum TracerError {
    /// Database-related error
    Database(String),
    /// Execution-related error
    Execution(String),
    /// System call-related error
    System(String),
}

impl StdError for TracerError {}

impl Display for TracerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TracerError::Database(msg) => write!(f, "Database error: {}", msg),
            TracerError::Execution(msg) => write!(f, "Execution error: {}", msg),
            TracerError::System(msg) => write!(f, "System error: {}", msg),
        }
    }
}
