//! Traits for configuring EVM specifics.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

#[cfg(feature = "std")]
use crate::execute::{BasicBlockBuilder, BasicBlockExecutor};
use alloc::string::String;
use alloy_eips::eip4895::Withdrawals;
use alloy_primitives::{Address, Bytes, B256};
use core::{error::Error, fmt::Debug};
use reth_primitives_traits::{
    BlockTy, HeaderTy, NodePrimitives, ReceiptTy, SealedBlock, SealedHeader, TxTy,
};

pub use evm2::{
    debug_unreachable,
    evm::{Database, DynDatabase},
};

/// Cached database adapters for payload building.
pub mod cached;
/// Database adapters for EVM execution.
pub mod database;
pub mod either;
/// EVM environment configuration.
pub mod execute;
/// precompile cache provider.
#[cfg(feature = "std")]
pub mod precompile_cache;

mod aliases;
pub use aliases::*;
pub use execute::{
    BlockAssembler, BlockAssemblerInput, BlockBuilder, BlockBuilderOutcome, BlockExecutionError,
    BlockExecutionOutput, BlockExecutor, BlockExecutorFactory, BlockTransactionResult,
    BlockValidationError, CommitChanges, Evm, ExecutableTxFor, ExecutableTxParts, Executor,
    ExecutorTx, FromRecoveredTx, FromTxWithEncoded, GasOutput, InternalBlockExecutionError,
    IntoTxEnv, InvalidTxError, ReceiptBuilder, ReceiptBuilderCtx, RecoveredTx, TransactionEnvMut,
    WithTxEnv,
};
pub use reth_execution_types::{OnStateHook, StateUpdate};
pub use reth_trie_common::bundle_state::BundleState;

/// Transaction validation limits resolved for an EVM environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EvmTransactionValidationLimits {
    /// Maximum contract creation initcode size.
    pub max_initcode_size: usize,
    /// Transaction gas limit cap. `0` disables the txpool-level cap check.
    pub tx_gas_limit_cap: u64,
}

/// Transaction validation gas rules resolved for an EVM environment.
#[derive(Debug, Clone, Copy)]
pub struct EvmTransactionValidationGasRules {
    /// The configured native gas schedule and enabled features.
    pub version: evm2::Version,
}

/// Transaction validation gas resolved for a transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EvmTransactionValidationGas {
    /// Transaction intrinsic gas.
    pub intrinsic_gas: u64,
    /// Transaction floor gas.
    pub floor_gas: u64,
}

impl EvmTransactionValidationGasRules {
    /// Returns the intrinsic regular gas cap, or zero when EIP-8037 is disabled.
    pub const fn regular_gas_cap(&self) -> u64 {
        if self.version.feature(evm2::EvmFeatures::EIP8037) {
            self.version.tx_gas_limit_cap
        } else {
            0
        }
    }

    /// Calculates validation gas with the same native rules used during execution.
    #[expect(clippy::too_many_arguments)]
    pub fn calculate(
        &self,
        caller: alloy_primitives::Address,
        to: alloy_primitives::TxKind,
        value: alloy_primitives::U256,
        input: &alloy_primitives::Bytes,
        access_list_accounts: u64,
        access_list_storage_keys: u64,
        authorization_list_len: u64,
    ) -> EvmTransactionValidationGas {
        let (mut intrinsic_gas, floor_gas) = evm2::ethereum::intrinsic_and_floor_gas(
            &self.version,
            caller,
            to,
            input,
            access_list_accounts,
            access_list_storage_keys,
            value,
        );
        if self.version.feature(evm2::EvmFeatures::EIP7702) {
            intrinsic_gas += authorization_list_len *
                u64::from(
                    self.version.gas_params.get(evm2::version::GasId::TxEip7702PerEmptyAccountCost),
                );
        }
        EvmTransactionValidationGas { intrinsic_gas, floor_gas }
    }
}

/// Resolved EVM environment data needed by the EVM execution path.
pub trait EvmEnv: Debug + Clone + Send + Sync + 'static {
    /// Runtime EVM type family.
    type EvmTypes: evm2::EvmTypes;

    /// Returns the active EVM specification.
    fn spec_id(&self) -> <Self::EvmTypes as evm2::EvmTypesHost>::SpecId;

    /// Returns the active chain ID.
    fn chain_id(&self) -> u64;

    /// Returns the configured block environment.
    fn block_env(&self) -> &evm2::env::BlockEnv<Self::EvmTypes>;

    /// Returns the configured block environment mutably.
    fn block_env_mut(&mut self) -> &mut evm2::env::BlockEnv<Self::EvmTypes>;

    /// Returns the active EVM version.
    fn version(&self) -> &evm2::Version;

    /// Returns the active EVM version mutably.
    fn version_mut(&mut self) -> &mut evm2::Version;

    /// Returns the block base fee resolved for this environment.
    fn block_base_fee(&self) -> u64;

    /// Returns the block blob base fee resolved for this environment.
    fn block_blob_base_fee(&self) -> u64;

    /// Returns transaction validation limits active in this environment.
    fn transaction_validation_limits(&self) -> EvmTransactionValidationLimits;

    /// Returns transaction validation gas rules active in this environment.
    fn transaction_validation_gas_rules(&self) -> EvmTransactionValidationGasRules;

    /// Returns whether the block uses independent regular and state gas capacity.
    fn uses_separate_block_gas(&self) -> bool {
        false
    }

    /// Returns the transaction limit used to reserve regular block gas.
    fn regular_gas_limit_cap(&self) -> u64 {
        u64::MAX
    }

    /// Returns this environment with transaction nonce checks disabled.
    fn with_nonce_check_disabled(self) -> Self;

    /// Returns this environment with transaction balance checks disabled.
    fn with_balance_check_disabled(self) -> Self;
}

#[cfg(feature = "std")]
mod engine;
#[cfg(feature = "std")]
pub use engine::{ConfigureEngineEvm, ConvertTx, ExecutableTxIterator, ExecutableTxTuple};
mod sender_recovery;
pub use sender_recovery::SenderRecoveryCache;

#[cfg(feature = "metrics")]
pub mod metrics;
pub mod noop;
/// Helper types for execution witness generation.
#[cfg(feature = "witness")]
pub mod witness;

/// A complete configuration of EVM for Reth.
///
/// This trait encapsulates complete configuration required for transaction execution and block
/// execution/building, providing a unified interface for EVM operations.
///
/// # Architecture Overview
///
/// The EVM abstraction consists of the following layers:
///
/// 1. **[`Evm`] (created by [`BlockExecutorFactory::evm_with_env`])**: The core EVM implementation
///    responsible for executing individual transactions and producing outputs including state
///    changes, logs, gas usage, etc. The factory that configures it is
///    [`BlockExecutorFactory::EvmFactory`].
///
/// 2. **[`BlockExecutor`] (created by [`BlockExecutorFactory::create_executor`])**: A higher-level
///    component that operates on top of [`Evm`] to execute entire blocks. This involves:
///    - Executing all transactions in sequence
///    - Building receipts from transaction outputs
///    - Applying block rewards to the beneficiary
///    - Executing system calls (e.g., EIP-4788 beacon root updates)
///    - Managing state changes and bundle accumulation
///
/// 3. **[`BlockAssembler`]**: Responsible for assembling valid blocks from executed transactions.
///    It takes the output from [`BlockExecutor`] along with execution context and produces a
///    complete block ready for inclusion in the chain.
///
/// # Usage Patterns
///
/// The abstraction supports two primary use cases:
///
/// ## 1. Executing Externally Provided Blocks (e.g., during sync)
///
/// ```rust,ignore
/// use reth_evm::ConfigureEvm;
///
/// // Execute a received block
/// let mut executor = evm_config.executor(state_db);
/// let output = executor.execute(&block)?;
///
/// // Access the execution results
/// println!("Gas used: {}", output.result.gas_used);
/// println!("Receipts: {:?}", output.result.receipts);
/// ```
///
/// ## 2. Building New Blocks (e.g., payload building)
///
/// Payload building is slightly different as it doesn't have the block's header yet, but rather
/// attributes for the block's environment, such as timestamp, fee recipient, and randomness value.
/// The block's header will be the outcome of the block building process.
///
/// ```rust,ignore
/// use reth_evm::{ConfigureEvm, NextBlockEnvAttributes};
///
/// // Create attributes for the next block
/// let attributes = NextBlockEnvAttributes {
///     timestamp: current_time + 12,
///     suggested_fee_recipient: beneficiary_address,
///     prev_randao: randomness_value,
///     gas_limit: 30_000_000,
///     withdrawals: Some(withdrawals),
///     parent_beacon_block_root: Some(beacon_root),
///     extra_data: Bytes::new(),
///     slot_number: None,
/// };
///
/// // Build a new block on top of parent
/// let mut builder = evm_config.builder_for_next_block(
///     state_db,
///     &parent_header,
///     attributes
/// )?;
///
/// // Apply pre-execution changes (e.g., beacon root update)
/// builder.apply_pre_execution_changes()?;
///
/// // Execute transactions
/// for tx in pending_transactions {
///     match builder.execute_transaction(tx) {
///         Ok(gas_output) => {
///             println!("Transaction executed, gas used: {}", gas_output.tx_gas_used());
///         }
///         Err(e) => {
///             println!("Transaction failed: {:?}", e);
///         }
///     }
/// }
///
/// // Finish block building and get the outcome (block)
/// let outcome = builder.finish(state_provider, None)?;
/// let block = outcome.block;
/// ```
///
/// # Key Components
///
/// ## [`NextBlockEnvCtx`]
///
/// Contains attributes needed to configure the next block that cannot be derived from the
/// parent block alone. This includes data typically provided by the consensus layer:
/// - `timestamp`: Block timestamp
/// - `suggested_fee_recipient`: Beneficiary address
/// - `prev_randao`: Randomness value
/// - `gas_limit`: Block gas limit
/// - `withdrawals`: Consensus layer withdrawals
/// - `parent_beacon_block_root`: EIP-4788 beacon root
///
/// ## [`BlockAssembler`]
///
/// Takes the execution output and produces a complete block. It receives:
/// - Transaction execution results (receipts, gas used)
/// - Final state root after all executions
/// - Bundle state with all changes
/// - Execution context and environment
///
/// The assembler is responsible for:
/// - Setting the correct block header fields
/// - Including executed transactions
/// - Setting gas used and receipts root
/// - Applying any chain-specific rules
///
/// [`ExecutionCtx`]: BlockExecutorFactory::ExecutionCtx
/// [`NextBlockEnvCtx`]: ConfigureEvm::NextBlockEnvCtx
#[auto_impl::auto_impl(&, Arc)]
pub trait ConfigureEvm: Clone + Debug + Send + Sync + Unpin {
    /// The primitives type used by the EVM.
    type Primitives: NodePrimitives;

    /// The error type that is returned by [`Self::next_evm_env`].
    type Error: Error + Send + Sync + 'static;

    /// Context required for configuring next block environment.
    ///
    /// Contains values that can't be derived from the parent block.
    type NextBlockEnvCtx: Debug + Clone;

    /// Configured [`BlockExecutorFactory`], contains the EVM factory internally.
    type BlockExecutorFactory: crate::execute::BlockExecutorFactory<
        Transaction = TxTy<Self::Primitives>,
        Receipt = ReceiptTy<Self::Primitives>,
        EvmTypes: evm2::EvmTypes<
            Tx: crate::execute::FromTxWithEncoded<TxTy<Self::Primitives>>
                    + crate::execute::TransactionEnvMut,
        >,
    >;

    /// A type that knows how to build a block.
    #[cfg(feature = "std")]
    type BlockAssembler: crate::execute::BlockAssembler<
        Self::BlockExecutorFactory,
        Block = BlockTy<Self::Primitives>,
    >;

    /// Returns reference to the configured [`BlockExecutorFactory`].
    #[cfg(feature = "std")]
    fn block_executor_factory(&self) -> &Self::BlockExecutorFactory;

    /// Returns reference to the configured [`BlockAssembler`].
    #[cfg(feature = "std")]
    fn block_assembler(&self) -> &Self::BlockAssembler;

    /// Creates a new [`EvmEnv`] for the given header.
    fn evm_env(&self, header: &HeaderTy<Self::Primitives>) -> Result<EvmEnvFor<Self>, Self::Error>;

    /// Returns the configured [`EvmEnv`] for `parent + 1` block.
    ///
    /// This is intended for usage in block building after the merge and requires additional
    /// attributes that can't be derived from the parent block: attributes that are determined by
    /// the CL, such as the timestamp, suggested fee recipient, and randomness value.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let evm_env = evm_config.next_evm_env(&parent_header, &attributes)?;
    /// // evm_env now contains:
    /// // - Correct spec ID based on timestamp and block number
    /// // - Block environment with next block's parameters
    /// // - Configuration like chain ID and blob parameters
    /// ```
    fn next_evm_env(
        &self,
        parent: &HeaderTy<Self::Primitives>,
        attributes: &Self::NextBlockEnvCtx,
    ) -> Result<EvmEnvFor<Self>, Self::Error>;

    /// Returns the configured [`BlockExecutorFactory::ExecutionCtx`] for a given block.
    fn context_for_block<'a>(
        &self,
        block: &'a SealedBlock<BlockTy<Self::Primitives>>,
    ) -> Result<ExecutionCtxFor<'a, Self>, Self::Error>
    where
        Self: 'a;

    /// Returns the configured [`BlockExecutorFactory::ExecutionCtx`] for `parent + 1`
    /// block.
    fn context_for_next_block(
        &self,
        parent: &SealedHeader<HeaderTy<Self::Primitives>>,
        attributes: Self::NextBlockEnvCtx,
    ) -> Result<ExecutionCtxFor<'_, Self>, Self::Error>;

    /// Returns a transaction environment from a transaction.
    fn tx_env(&self, transaction: impl IntoTxEnv<TxEnvFor<Self>>) -> TxEnvFor<Self> {
        transaction.into_tx_env()
    }

    /// Provides a reference to the configured EVM factory state.
    #[cfg(feature = "std")]
    fn evm_factory(&self) -> &EvmFactoryFor<Self> {
        self.block_executor_factory().evm_factory()
    }

    /// Returns a config with JIT support enabled for subsequently created EVMs, if supported.
    ///
    /// This is one of three gates required before an EVM can execute JIT-compiled code: the binary
    /// must be built with the `jit` feature, runtime compilation must be enabled by `--jit` or the
    /// `reth_jit` RPC method, and this local support flag must be enabled for the config that
    /// creates the EVM.
    #[auto_impl(keep_default_for(&, Arc))]
    fn with_jit_support_enabled(self, _enabled: bool) -> Self
    where
        Self: Sized,
    {
        self
    }

    /// Returns a config with local JIT support enabled for subsequently created EVMs, if supported.
    #[auto_impl(keep_default_for(&, Arc))]
    fn with_jit_support(self) -> Self
    where
        Self: Sized,
    {
        self.with_jit_support_enabled(true)
    }

    /// Creates an EVM whose precompile results are cached in `cache_map`.
    ///
    /// Only the engine calls this, for the EVMs it executes and prewarms blocks with. Configs that
    /// do not support precompile caching create an uncached EVM. `metrics_enabled` selects whether
    /// the cache records hit, miss, and size metrics.
    #[cfg(feature = "std")]
    fn evm_with_precompile_cache<'a, DB>(
        &self,
        db: DB,
        evm_env: EvmEnvFor<Self>,
        _cache_map: precompile_cache::PrecompileCacheMap<SpecFor<Self>>,
        _metrics_enabled: bool,
    ) -> EvmFor<'a, Self>
    where
        DB: Database + 'a,
    {
        self.evm_with_env(db, evm_env)
    }

    /// Returns the JIT backend, if supported.
    fn jit_backend(&self) -> Option<&dyn JitBackend> {
        None
    }

    /// Returns a new [`Executor`] for executing blocks.
    ///
    /// The executor processes complete blocks including:
    /// - All transactions in order
    /// - Block rewards and fees
    /// - Block level system calls
    /// - State transitions
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// // Create an executor
    /// let mut executor = evm_config.executor(state_db);
    ///
    /// // Execute a single block
    /// let output = executor.execute(&block)?;
    ///
    /// // Execute multiple blocks
    /// let batch_output = executor.execute_batch(&blocks)?;
    /// ```
    #[auto_impl(keep_default_for(&, Arc))]
    fn executor<DB>(
        &self,
        db: DB,
    ) -> impl Executor<DB, Primitives = Self::Primitives, Error = BlockExecutionError>
    where
        DB: Database,
    {
        #[cfg(feature = "std")]
        {
            BasicBlockExecutor::new(self, db)
        }

        #[cfg(not(feature = "std"))]
        {
            let _ = db;
            crate::execute::UnsupportedExecutor::default()
        }
    }

    /// Returns a new [`BasicBlockExecutor`].
    #[auto_impl(keep_default_for(&, Arc))]
    fn batch_executor<DB>(
        &self,
        db: DB,
    ) -> impl Executor<DB, Primitives = Self::Primitives, Error = BlockExecutionError>
    where
        DB: Database,
    {
        #[cfg(feature = "std")]
        {
            BasicBlockExecutor::new(self, db)
        }

        #[cfg(not(feature = "std"))]
        {
            let _ = db;
            crate::execute::UnsupportedExecutor::default()
        }
    }

    /// Creates a block executor for the given block.
    #[cfg(feature = "std")]
    fn executor_for_block<'a, DB>(
        &'a self,
        db: DB,
        block: &'a SealedBlock<BlockTy<Self::Primitives>>,
    ) -> Result<crate::BlockExecutorFor<'a, Self>, Self::Error>
    where
        Self: 'a,
        DB: Database + 'a,
    {
        let evm = self.evm_for_block(db, block.header())?;
        let ctx = self.context_for_block(block)?;
        Ok(self.block_executor_factory().create_executor(evm, ctx))
    }

    /// Creates an EVM instance for single-transaction execution with the configured environment.
    #[cfg(feature = "std")]
    #[auto_impl(keep_default_for(&, Arc))]
    fn evm_with_env<'a, DB>(&self, db: DB, evm_env: EvmEnvFor<Self>) -> EvmFor<'a, Self>
    where
        DB: Database + 'a,
    {
        self.block_executor_factory().evm_with_database(db, evm_env)
    }

    /// Creates an EVM instance for the given block.
    #[cfg(feature = "std")]
    fn evm_for_block<'a, DB>(
        &self,
        db: DB,
        header: &HeaderTy<Self::Primitives>,
    ) -> Result<EvmFor<'a, Self>, Self::Error>
    where
        DB: Database + 'a,
    {
        let evm_env = self.evm_env(header)?;
        Ok(self.evm_with_env(db, evm_env))
    }

    /// Creates a [`BlockBuilder`]. Should be used when building a new block.
    ///
    /// Block builder wraps an inner [`BlockExecutor`] and has a similar interface. Builder
    /// collects all of the executed transactions, and once [`BlockBuilder::finish`] is called, it
    /// invokes the configured [`BlockAssembler`] to create a block.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// // Create a builder with specific EVM configuration
    /// let evm = evm_config.evm_with_env(state_db, evm_env.clone());
    /// let ctx = evm_config.context_for_next_block(&parent, attributes);
    /// let builder = evm_config.create_block_builder(evm, evm_env, &parent, ctx);
    /// ```
    #[cfg(feature = "std")]
    fn create_block_builder<'a>(
        &'a self,
        evm: EvmFor<'a, Self>,
        evm_env: EvmEnvFor<Self>,
        parent: &'a SealedHeader<HeaderTy<Self::Primitives>>,
        ctx: ExecutionCtxFor<'a, Self>,
    ) -> impl BlockBuilder<Primitives = Self::Primitives, Executor = crate::BlockExecutorFor<'a, Self>>
    where
        Self: 'a,
    {
        self.create_block_builder_with(evm, evm_env, parent, ctx, |executor| executor)
    }

    /// Creates a [`BlockBuilder`] like [`ConfigureEvm::create_block_builder`], with a customized
    /// executor.
    #[cfg(feature = "std")]
    fn create_block_builder_with<'a, E>(
        &'a self,
        evm: EvmFor<'a, Self>,
        evm_env: EvmEnvFor<Self>,
        parent: &'a SealedHeader<HeaderTy<Self::Primitives>>,
        ctx: ExecutionCtxFor<'a, Self>,
        f: impl FnOnce(BlockExecutorFor<'a, Self>) -> E,
    ) -> impl BlockBuilder<Primitives = Self::Primitives, Executor = E>
    where
        Self: 'a,
        E: BlockExecutor<
            Evm = EvmFor<'a, Self>,
            Transaction = TxTy<Self::Primitives>,
            Receipt = ReceiptTy<Self::Primitives>,
        >,
    {
        BasicBlockBuilder {
            executor: f(self.block_executor_factory().create_executor(evm, ctx.clone())),
            evm_env,
            transactions: Default::default(),
            ctx,
            parent,
            assembler: self.block_assembler(),
        }
    }

    /// Creates a [`BlockBuilder`] for building of a new block. This is a helper to invoke
    /// [`ConfigureEvm::create_block_builder`].
    ///
    /// This is the primary method for building new blocks. It combines:
    /// 1. Creating the EVM environment for the next block
    /// 2. Setting up the execution context from attributes
    /// 3. Initializing the block builder with proper configuration
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// // Build a block with specific attributes
    /// let mut builder = evm_config.builder_for_next_block(
    ///     state_db,
    ///     &parent_header,
    ///     attributes
    /// )?;
    ///
    /// // Execute system calls (e.g., beacon root update)
    /// builder.apply_pre_execution_changes()?;
    ///
    /// // Execute transactions
    /// for tx in transactions {
    ///     builder.execute_transaction(tx)?;
    /// }
    ///
    /// // Complete block building
    /// let outcome = builder.finish(state_provider, None)?;
    /// ```
    #[cfg(feature = "std")]
    fn builder_for_next_block<'a, DB>(
        &'a self,
        db: DB,
        parent: &'a SealedHeader<HeaderTy<Self::Primitives>>,
        attributes: Self::NextBlockEnvCtx,
    ) -> Result<
        impl BlockBuilder<Primitives = Self::Primitives, Executor = crate::BlockExecutorFor<'a, Self>>,
        Self::Error,
    >
    where
        Self: 'a,
        DB: Database + 'a,
    {
        let evm_env = self.next_evm_env(parent, &attributes)?;
        let evm = self.evm_with_env(db, evm_env.clone());
        let ctx = self.context_for_next_block(parent, attributes)?;
        Ok(self.create_block_builder(evm, evm_env, parent, ctx))
    }

    /// Applies block-level state changes required before transaction execution using the configured
    /// block executor.
    #[cfg(feature = "std")]
    fn pre_block_state_changes<'a, DB>(
        &'a self,
        db: DB,
        evm_env: EvmEnvFor<Self>,
        ctx: ExecutionCtxFor<'a, Self>,
    ) -> Result<BundleState, Box<dyn Error + Send + Sync>>
    where
        Self: 'a,
        DB: DynDatabase + 'a,
    {
        let factory = self.block_executor_factory();
        let evm = factory.evm_with_env(db, evm_env);
        let mut executor = factory.create_executor(evm, ctx);
        executor
            .apply_pre_execution_changes()
            .map_err(|err| -> Box<dyn Error + Send + Sync> { Box::new(err) })?;
        Ok(executor.into_state())
    }

    /// Returns whether engine validation may commit transaction results from speculative prewarm
    /// execution instead of executing the transactions again.
    ///
    /// Return `true` only if all of these hold, see also [`Self::prewarm_tx_result`]:
    /// - the block executor executes a transaction by checking that its gas limit, capped by the
    ///   transaction gas limit cap, fits into the block gas limit minus the last receipt's
    ///   cumulative gas used, and then running [`Evm::transact`] with the transaction environment;
    /// - [`Evm::transact`] performs the standard caller nonce and balance validation; speculative
    ///   results additionally reject the largest nonce before reuse;
    /// - EVMs created by this configuration with JIT support disabled call inspector hooks for
    ///   every executed instruction;
    /// - execution depends only on recorded database reads and the transaction and block
    ///   environments reproduced during prewarming, with no unrecorded external state.
    ///
    /// [`Evm::transact`]: Evm::transact
    fn reuses_prewarm_results(&self) -> bool {
        false
    }

    /// Wraps a speculative prewarm execution result of `tx` into the result consumed by
    /// [`BlockExecutor::commit_transaction`].
    ///
    /// Only called if [`Self::reuses_prewarm_results`] returns `true`.
    /// Returning `None` rejects and consumes this speculative result; the engine executes the
    /// transaction normally instead.
    #[cfg(feature = "std")]
    fn prewarm_tx_result<'a>(
        &self,
        tx: &TxTy<Self::Primitives>,
        result: TxResultWithStateFor<Self>,
    ) -> Option<TxExecutionResultFor<'a, Self>>
    where
        Self: 'a,
    {
        let _ = (tx, result);
        None
    }

    /// Returns the predicted environment of the block after `parent` with `beneficiary`, in which
    /// engine validation speculatively executes pool transactions before the block arrives.
    ///
    /// Validation reuses such results only for blocks whose environment matches the predicted
    /// one, apart from the beneficiary, timestamp, prevrandao and gas limit, which the
    /// transactions must not have observed. Only called if [`Self::reuses_prewarm_results`]
    /// returns `true`. Returns `None` if the environment cannot be predicted, which disables
    /// recording pool transaction results. Custom block environment extensions must remain
    /// identical to canonical execution; the engine compares only standard environment fields.
    fn speculative_next_evm_env(
        &self,
        parent: &SealedHeader<HeaderTy<Self::Primitives>>,
        beneficiary: Address,
    ) -> Option<EvmEnvFor<Self>> {
        let _ = (parent, beneficiary);
        None
    }
}

/// JIT backend controls exposed by an EVM configuration.
pub trait JitBackend: Send + Sync {
    /// Enables or disables JIT compilation.
    fn set_enabled(&self, enabled: bool) -> Result<(), String>;

    /// Pauses JIT helper execution while keeping queueing and resident compiled code available.
    fn pause(&self);

    /// Resumes background JIT work.
    fn resume(&self);

    /// Clears JIT runtime state.
    fn clear(&self);
}

/// Represents additional attributes required to configure the next block.
///
/// This struct contains all the information needed to build a new block that cannot be
/// derived from the parent block header alone. These attributes are typically provided
/// by the consensus layer (CL) through the Engine API during payload building.
///
/// # Relationship with [`ConfigureEvm`] and [`BlockAssembler`]
///
/// The flow for building a new block involves:
///
/// 1. **Receive attributes** from the consensus layer containing:
///    - Timestamp for the new block
///    - Fee recipient (coinbase/beneficiary)
///    - Randomness value (prevRandao)
///    - Withdrawals to process
///    - Parent beacon block root for EIP-4788
///
/// 2. **Configure EVM environment** using these attributes: ```rust,ignore let evm_env =
///    evm_config.next_evm_env(&parent, &attributes)?; ```
///
/// 3. **Build the block** with transactions: ```rust,ignore let mut builder =
///    evm_config.builder_for_next_block( state, &parent, attributes )?; ```
///
/// 4. **Assemble the final block** using [`BlockAssembler`] which takes:
///    - Execution results from all transactions
///    - The attributes used during execution
///    - Final state root after all changes
///
/// This design cleanly separates:
/// - **Configuration** (what parameters to use) - handled by `NextBlockEnvAttributes`
/// - **Execution** (running transactions) - handled by [`BlockExecutor`]
/// - **Assembly** (creating the final block) - handled by [`BlockAssembler`]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NextBlockEnvAttributes {
    /// The timestamp of the next block.
    pub timestamp: u64,
    /// The suggested fee recipient for the next block.
    pub suggested_fee_recipient: Address,
    /// The randomness value for the next block.
    pub prev_randao: B256,
    /// Block gas limit.
    pub gas_limit: u64,
    /// The parent beacon block root.
    pub parent_beacon_block_root: Option<B256>,
    /// Withdrawals
    pub withdrawals: Option<Withdrawals>,
    /// Optional extra data.
    pub extra_data: Bytes,
    /// Optional slot number for post-Amsterdam payloads.
    pub slot_number: Option<u64>,
}
