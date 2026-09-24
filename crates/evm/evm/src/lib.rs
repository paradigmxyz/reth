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
use alloy_consensus::Transaction;
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
/// Cancellation markers for EVM execution work.
pub use reth_revm::cancelled;
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
    BlockValidationError, CommitChanges, Evm, EvmError, ExecutableTxFor, ExecutableTxParts,
    Executor, ExecutorTx, FromRecoveredTx, FromTxWithEncoded, GasOutput,
    InternalBlockExecutionError, IntoTxEnv, InvalidTxError, ReceiptBuilder, ReceiptBuilderCtx,
    RecoveredTx, WithTxEnv,
};
pub use reth_execution_types::EvmState;
pub use revm::database_interface::OnStateHook;

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
        let mut intrinsic_gas = evm2::ethereum::intrinsic_gas(
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
        let floor_gas = evm2::ethereum::floor_gas(
            &self.version,
            caller,
            to,
            input,
            access_list_accounts,
            access_list_storage_keys,
            value,
        );
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
#[cfg(any(test, feature = "test-utils"))]
/// test helpers for mocking executor
pub mod test_utils;
/// Helper types for execution witness generation.
#[cfg(feature = "witness")]
pub mod witness;

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
    type BlockExecutorFactory: crate::execute::BlockExecutorFactory<
        Transaction = TxTy<Self::Primitives>,
        Receipt = ReceiptTy<Self::Primitives>,
        EvmTypes: evm2::EvmTypes<
            Tx: crate::execute::FromTxWithEncoded<TxTy<Self::Primitives>> + Transaction + Clone,
        >,
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
    fn tx_env(&self, transaction: impl IntoTxEnv<TxEnvFor<Self>>) -> TxEnvFor<Self> {
        transaction.into_tx_env()
    }

    /// Provides a reference to the configured EVM factory state.
    #[cfg(feature = "std")]
    fn evm_factory(&self) -> &EvmFactoryFor<Self> {
        self.block_executor_factory().evm_factory()
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

    /// Enables precompile cache metrics for subsequently created EVMs, if supported.
    #[auto_impl(keep_default_for(&, Arc))]
    fn with_precompile_cache_metrics(self, _enabled: bool) -> Self
    where
        Self: Sized,
    {
        self
    }

    /// Returns the JIT backend, if supported.
    fn jit_backend(&self) -> Option<&dyn JitBackend> {
        None
    }

    /// Returns an executor for block execution over the provided database.
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

    /// Returns an executor for batch block execution over the provided database.
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
        _db: DB,
        _evm_env: EvmEnvFor<Self>,
        _block_number: u64,
        _ctx: ExecutionCtxFor<'a, Self>,
    ) -> Result<revm::database::BundleState, Box<dyn Error + Send + Sync>>
    where
        Self: 'a,
        DB: DynDatabase + 'a,
    {
        Ok(revm::database::BundleState::default())
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
