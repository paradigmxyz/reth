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
#[cfg(feature = "std")]
use alloc::boxed::Box;
use alloc::string::String;
use alloy_consensus::transaction::Recovered;
use alloy_eips::eip4895::Withdrawals;
use alloy_primitives::{Address, Bytes, B256};
use core::{error::Error, fmt::Debug};
use reth_primitives_traits::{BlockTy, HeaderTy, NodePrimitives, SealedBlock, SealedHeader, TxTy};

pub use evm2::{
    debug_unreachable,
    evm::{Database, DynDatabase},
};

/// Cached database adapters for payload building.
pub mod cached;
/// Cancellation markers for EVM execution work.
pub mod cancelled;
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
    BlockExecutionOutput, BlockExecutor, BlockExecutorFactory, BlockValidationError, CommitChanges,
    EvmError, ExecutableTxFor, ExecutableTxParts, ExecuteAndDiscard, Executor, ExecutorTx,
    GasOutput, InternalBlockExecutionError, InvalidTxError, RecoveredTx, WithTxEnv,
};
pub use reth_execution_types::ExecutionState;

/// Transaction validation limits resolved for an EVM environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EvmTransactionValidationLimits {
    /// Maximum contract creation initcode size.
    pub max_initcode_size: usize,
    /// Transaction gas limit cap. `0` disables the txpool-level cap check.
    pub tx_gas_limit_cap: u64,
}

/// Transaction validation gas rules resolved for an EVM environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EvmTransactionValidationGasRules {
    /// Base transaction gas.
    pub tx_base_gas: u64,
    /// Gas charged for create transactions.
    pub tx_create_gas: u64,
    /// Gas charged per zero calldata byte.
    pub tx_data_zero_gas: u64,
    /// Gas charged per non-zero calldata byte.
    pub tx_data_non_zero_gas: u64,
    /// Gas charged per access list address.
    pub tx_access_list_address_gas: u64,
    /// Gas charged per access list storage key.
    pub tx_access_list_storage_key_gas: u64,
    /// Floor gas tokens charged per access-list byte.
    pub tx_access_list_floor_byte_multiplier: u64,
    /// Gas charged per initcode word.
    pub tx_initcode_word_gas: u64,
    /// Base gas used for calldata floor gas.
    pub tx_floor_gas_base: u64,
    /// Floor gas charged per token. Zero disables floor gas.
    pub tx_floor_gas_per_token: u64,
    /// Token multiplier for non-zero calldata bytes.
    pub tx_floor_gas_non_zero_token_multiplier: u64,
    /// EIP-7702 gas charged per authorization.
    pub tx_eip7702_per_empty_account_cost: u64,
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
    /// Calculates transaction validation gas for the resolved rules.
    pub fn calculate(
        self,
        input: &[u8],
        is_create: bool,
        access_list_accounts: u64,
        access_list_storage_keys: u64,
        authorization_list_len: u64,
    ) -> EvmTransactionValidationGas {
        let (zero_data_len, non_zero_data_len) =
            input.iter().fold((0u64, 0u64), |(zero, non_zero), byte| {
                if *byte == 0 {
                    (zero + 1, non_zero)
                } else {
                    (zero, non_zero + 1)
                }
            });

        let mut intrinsic_gas = self
            .tx_base_gas
            .saturating_add(zero_data_len.saturating_mul(self.tx_data_zero_gas))
            .saturating_add(non_zero_data_len.saturating_mul(self.tx_data_non_zero_gas))
            .saturating_add(access_list_accounts.saturating_mul(self.tx_access_list_address_gas))
            .saturating_add(
                access_list_storage_keys.saturating_mul(self.tx_access_list_storage_key_gas),
            )
            .saturating_add(
                authorization_list_len.saturating_mul(self.tx_eip7702_per_empty_account_cost),
            );

        if is_create {
            intrinsic_gas = intrinsic_gas.saturating_add(self.tx_create_gas).saturating_add(
                self.tx_initcode_word_gas
                    .saturating_mul(u64::try_from(input.len().div_ceil(32)).unwrap_or(u64::MAX)),
            );
        }

        let floor_gas = if self.tx_floor_gas_per_token == 0 {
            0
        } else {
            let access_list_tokens = access_list_accounts
                .saturating_mul(20)
                .saturating_add(access_list_storage_keys.saturating_mul(32))
                .saturating_mul(self.tx_access_list_floor_byte_multiplier);
            let calldata_tokens = zero_data_len.saturating_add(
                non_zero_data_len.saturating_mul(self.tx_floor_gas_non_zero_token_multiplier),
            );

            self.tx_floor_gas_base.saturating_add(
                access_list_tokens
                    .saturating_add(calldata_tokens)
                    .saturating_mul(self.tx_floor_gas_per_token),
            )
        };

        EvmTransactionValidationGas { intrinsic_gas, floor_gas }
    }
}

/// Resolved EVM environment data needed by the EVM execution path.
pub trait EvmEnv: Debug + Clone + Send + Sync + 'static {
    /// Returns the resolved EVM block environment.
    fn block_env(&self) -> &evm2::env::BlockEnv;

    /// Returns the block base fee resolved for this environment.
    fn block_base_fee(&self) -> u64;

    /// Returns the block blob base fee resolved for this environment.
    fn block_blob_base_fee(&self) -> u64;

    /// Returns transaction validation limits active in this environment.
    fn transaction_validation_limits(&self) -> EvmTransactionValidationLimits;

    /// Returns transaction validation gas rules active in this environment.
    fn transaction_validation_gas_rules(&self) -> EvmTransactionValidationGasRules;

    /// Returns this environment with transaction nonce checks disabled.
    fn with_nonce_check_disabled(self) -> Self;

    /// Returns this environment with transaction balance checks disabled.
    fn with_balance_check_disabled(self) -> Self;
}

#[cfg(feature = "std")]
mod engine;
#[cfg(feature = "std")]
pub use engine::{ConfigureEngineEvm, ConvertTx, ExecutableTxIterator, ExecutableTxTuple};

#[cfg(feature = "metrics")]
pub mod metrics;
pub mod noop;
#[cfg(any(test, feature = "test-utils"))]
/// test helpers for mocking executor
pub mod test_utils;

/// A complete configuration of EVM for Reth.
///
/// This trait encapsulates configuration required for EVM, block execution, and block assembly.
#[auto_impl::auto_impl(&, Arc)]
pub trait ConfigureEvm: Clone + Debug + Send + Sync + Unpin {
    /// The primitives type used by the EVM.
    type Primitives: NodePrimitives;

    /// The error type that is returned by environment builders.
    type Error: Error + Send + Sync + 'static;

    /// Context required for configuring next block environment.
    type NextBlockEnvCtx: Debug + Clone;

    /// Configured block executor factory.
    type BlockExecutorFactory: for<'a> crate::execute::BlockExecutorFactory<
        Primitives = Self::Primitives,
        Transaction: From<Recovered<TxTy<Self::Primitives>>>,
    >;

    /// Configured block assembler.
    #[cfg(feature = "std")]
    type BlockAssembler: crate::execute::BlockAssembler<
        Self::BlockExecutorFactory,
        Block = BlockTy<Self::Primitives>,
    >;

    /// Returns the configured block executor factory.
    #[cfg(feature = "std")]
    fn block_executor_factory(&self) -> &Self::BlockExecutorFactory;

    /// Returns the configured block assembler.
    #[cfg(feature = "std")]
    fn block_assembler(&self) -> &Self::BlockAssembler;

    /// Creates a new EVM environment for the given header.
    fn evm_env(&self, header: &HeaderTy<Self::Primitives>) -> Result<EvmEnvFor<Self>, Self::Error>;

    /// Returns the configured EVM environment for `parent + 1` block.
    fn next_evm_env(
        &self,
        parent: &HeaderTy<Self::Primitives>,
        attributes: &Self::NextBlockEnvCtx,
    ) -> Result<EvmEnvFor<Self>, Self::Error>;

    /// Returns the configured execution context for a given block.
    fn context_for_block<'a>(
        &self,
        block: &'a SealedBlock<BlockTy<Self::Primitives>>,
    ) -> Result<ExecutionCtxFor<'a, Self>, Self::Error>
    where
        Self: 'a;

    /// Returns the configured execution context for `parent + 1` block.
    fn context_for_next_block(
        &self,
        parent: &SealedHeader<HeaderTy<Self::Primitives>>,
        attributes: Self::NextBlockEnvCtx,
    ) -> Result<ExecutionCtxFor<'_, Self>, Self::Error>;

    /// Returns a transaction environment from a transaction.
    fn tx_env(&self, transaction: impl Into<TxEnvFor<Self>>) -> TxEnvFor<Self> {
        transaction.into()
    }

    /// Returns a config with JIT support enabled for subsequently created EVMs, if supported.
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

    /// Returns a config with precompile cache disabled for subsequently created EVMs, if
    /// supported.
    #[auto_impl(keep_default_for(&, Arc))]
    fn with_precompile_cache_disabled(self, _disabled: bool) -> Self
    where
        Self: Sized,
    {
        self
    }

    /// Returns the JIT backend, if supported.
    fn jit_backend(&self) -> Option<&dyn JitBackend> {
        None
    }

    /// Creates a block executor from a configured EVM and execution context.
    #[cfg(feature = "std")]
    fn create_executor<'a>(
        &'a self,
        evm: EvmFor<'a, Self>,
        ctx: ExecutionCtxFor<'a, Self>,
    ) -> BlockExecutorFor<'a, Self>
    where
        Self: 'a,
    {
        self.block_executor_factory().create_executor(evm, ctx)
    }

    /// Returns an executor for block execution over the provided database.
    #[auto_impl(keep_default_for(&, Arc))]
    fn executor<DB>(
        &self,
        db: DB,
    ) -> impl Executor<Primitives = Self::Primitives, Error = BlockExecutionError>
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

    /// Returns an executor for batch block execution over the provided database.
    #[auto_impl(keep_default_for(&, Arc))]
    fn batch_executor<DB>(
        &self,
        db: DB,
    ) -> impl Executor<Primitives = Self::Primitives, Error = BlockExecutionError>
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
        Ok(self.create_executor(evm, ctx))
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

    /// Creates a block builder for a configured EVM and execution context.
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
        BasicBlockBuilder::new(
            self.block_executor_factory(),
            self.block_assembler(),
            evm,
            evm_env,
            parent,
            ctx,
        )
    }

    /// Creates a block builder for `parent + 1`.
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

    /// Applies block-level state changes required before transaction execution.
    #[cfg(feature = "std")]
    fn pre_block_state_changes<'a, DB>(
        &self,
        db: DB,
        evm_env: EvmEnvFor<Self>,
        block_number: u64,
        ctx: ExecutionCtxFor<'a, Self>,
    ) -> Result<ExecutionState, Box<dyn Error + Send + Sync>>
    where
        Self: 'a,
        DB: DynDatabase + 'a;
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
    /// Withdrawals.
    pub withdrawals: Option<Withdrawals>,
    /// Optional extra data.
    pub extra_data: Bytes,
    /// Optional slot number for post-Amsterdam payloads.
    pub slot_number: Option<u64>,
}
