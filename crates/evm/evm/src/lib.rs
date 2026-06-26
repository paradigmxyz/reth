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
use crate::execute::HashedStateMode;
#[cfg(feature = "std")]
use crate::execute::{BasicBlockBuilder, BlockBuilder};
use crate::execute::{Executor, IntoTxEnv};
#[cfg(feature = "std")]
use alloc::boxed::Box;
use alloc::string::String;
use alloy_consensus::transaction::Recovered;
use alloy_eips::eip4895::Withdrawals;
use alloy_primitives::{Address, Bytes, B256};
use core::{error::Error, fmt::Debug};
use reth_primitives_traits::{BlockTy, HeaderTy, NodePrimitives, SealedBlock, SealedHeader, TxTy};

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

/// Resolved EVM environment data needed by the EVM execution path.
pub trait EvmEnv: Debug + Clone + Send + Sync + 'static {
    /// Returns the EVM block environment.
    fn block_env(&self) -> evm2::env::BlockEnv;

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

    /// Configured EVM environment type.
    type EvmEnv: crate::EvmEnv;

    /// Configured transaction environment type.
    type TxEnv: From<Recovered<TxTy<Self::Primitives>>> + Clone + Send + Sync + 'static;

    /// Execution context for a block or payload.
    type ExecutionCtx<'a>: Debug + Clone + Send
    where
        Self: 'a;

    /// Configured block executor factory.
    #[cfg(feature = "std")]
    type BlockExecutorFactory: crate::execute::BlockExecutorFactory<
        Primitives = Self::Primitives,
        Transaction = TxEnvFor<Self>,
        EvmEnv = EvmEnvFor<Self>,
    >;

    /// Configured block assembler.
    #[cfg(feature = "std")]
    type BlockAssembler: crate::execute::BlockAssembler<
        Self::BlockExecutorFactory,
        Block = BlockTy<Self::Primitives>,
    >;

    /// Executor returned for block execution over the provided database.
    type Executor<DB>: Executor<Primitives = Self::Primitives>
    where
        DB: evm2::evm::Database + Clone + 'static,
        DB::Error: core::error::Error + Send + Sync + 'static;

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
    fn context_for_next_block<'a>(
        &'a self,
        parent: &'a SealedHeader<HeaderTy<Self::Primitives>>,
        attributes: Self::NextBlockEnvCtx,
    ) -> Result<ExecutionCtxFor<'a, Self>, Self::Error>
    where
        Self: 'a;

    /// Returns the chain id used for transaction validation and the `CHAINID` opcode.
    fn chain_id(&self) -> u64;

    /// Returns the deposit contract address used to derive EIP-6110 deposit requests.
    fn deposit_contract_address(&self) -> Option<Address> {
        None
    }

    /// Returns a transaction environment from a transaction.
    fn tx_env(&self, transaction: impl IntoTxEnv<TxEnvFor<Self>>) -> TxEnvFor<Self> {
        transaction.into_tx_env()
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

    /// Returns the JIT backend, if supported.
    fn jit_backend(&self) -> Option<&dyn JitBackend> {
        None
    }

    /// Returns an executor for block execution over the provided database.
    fn executor<DB>(&self, db: DB) -> Self::Executor<DB>
    where
        DB: evm2::evm::Database + Clone + 'static,
        DB::Error: core::error::Error + Send + Sync + 'static;

    /// Returns an executor for batch block execution over the provided database.
    #[auto_impl(keep_default_for(&, Arc))]
    fn batch_executor<DB>(&self, db: DB) -> Self::Executor<DB>
    where
        DB: evm2::evm::Database + Clone + 'static,
        DB::Error: core::error::Error + Send + Sync + 'static,
    {
        self.executor(db)
    }

    /// Creates a configured block executor for active block execution.
    #[cfg(feature = "std")]
    fn create_executor<'a, DB>(
        &'a self,
        evm: evm2::Evm<evm2::BaseEvmTypes>,
        ctx: ExecutionCtxFor<'a, Self>,
        hashed_state_mode: HashedStateMode,
    ) -> <Self::BlockExecutorFactory as crate::execute::BlockExecutorFactory>::Executor<'a, DB>
    where
        Self: 'a,
        DB: evm2::evm::Database + Clone + 'static,
        DB::Error: core::error::Error + Send + Sync + 'static;

    /// Creates a block executor for the given block.
    #[cfg(feature = "std")]
    fn executor_for_block<'a, DB>(
        &'a self,
        db: DB,
        block: &'a SealedBlock<BlockTy<Self::Primitives>>,
        hashed_state_mode: HashedStateMode,
    ) -> Result<crate::BlockExecutorFor<'a, Self, DB>, Self::Error>
    where
        Self: 'a,
        Self::BlockExecutorFactory:
            crate::execute::BlockExecutorFactory<ExecutionCtx<'a> = ExecutionCtxFor<'a, Self>>,
        DB: evm2::evm::Database + Clone + 'static,
        DB::Error: core::error::Error + Send + Sync + 'static,
    {
        let evm = self.evm_for_block(evm2::evm::Db::new(db), block.header())?;
        let ctx = self.context_for_block(block)?;
        Ok(self.create_executor::<DB>(evm, ctx, hashed_state_mode))
    }

    /// Creates an EVM instance for single-transaction execution with the configured environment.
    #[cfg(feature = "std")]
    fn evm_with_env<DB>(&self, db: DB, evm_env: EvmEnvFor<Self>) -> evm2::Evm<evm2::BaseEvmTypes>
    where
        DB: evm2::evm::DynDatabase + 'static;

    /// Creates an EVM instance for the given block.
    #[cfg(feature = "std")]
    fn evm_for_block<DB>(
        &self,
        db: DB,
        header: &HeaderTy<Self::Primitives>,
    ) -> Result<evm2::Evm<evm2::BaseEvmTypes>, Self::Error>
    where
        DB: evm2::evm::DynDatabase + 'static,
    {
        let evm_env = self.evm_env(header)?;
        Ok(self.evm_with_env(db, evm_env))
    }

    /// Creates a block builder for a configured EVM and execution context.
    #[cfg(feature = "std")]
    fn create_block_builder<'a, DB>(
        &'a self,
        evm: evm2::Evm<evm2::BaseEvmTypes>,
        evm_env: EvmEnvFor<Self>,
        parent: &'a SealedHeader<HeaderTy<Self::Primitives>>,
        ctx: ExecutionCtxFor<'a, Self>,
        hashed_state_mode: HashedStateMode,
    ) -> impl BlockBuilder<Primitives = Self::Primitives, Executor = crate::BlockExecutorFor<'a, Self, DB>>
    where
        Self: 'a,
        Self::BlockExecutorFactory:
            crate::execute::BlockExecutorFactory<ExecutionCtx<'a> = ExecutionCtxFor<'a, Self>>,
        DB: evm2::evm::Database + Clone + 'static,
        DB::Error: core::error::Error + Send + Sync + 'static,
    {
        BasicBlockBuilder {
            executor: self.create_executor::<DB>(evm, ctx.clone(), hashed_state_mode),
            evm_env,
            transactions: Vec::new(),
            senders: Vec::new(),
            ctx,
            parent,
            assembler: self.block_assembler().clone(),
        }
    }

    /// Creates a block builder for `parent + 1`.
    #[cfg(feature = "std")]
    fn builder_for_next_block<'a, DB>(
        &'a self,
        db: DB,
        parent: &'a SealedHeader<HeaderTy<Self::Primitives>>,
        attributes: Self::NextBlockEnvCtx,
        hashed_state_mode: HashedStateMode,
    ) -> Result<
        impl BlockBuilder<
            Primitives = Self::Primitives,
            Executor = crate::BlockExecutorFor<'a, Self, DB>,
        >,
        Self::Error,
    >
    where
        Self: 'a,
        Self::BlockExecutorFactory:
            crate::execute::BlockExecutorFactory<ExecutionCtx<'a> = ExecutionCtxFor<'a, Self>>,
        DB: evm2::evm::Database + Clone + 'static,
        DB::Error: core::error::Error + Send + Sync + 'static,
    {
        let evm_env = self.next_evm_env(parent, &attributes)?;
        let evm = self.evm_with_env(evm2::evm::Db::new(db), evm_env.clone());
        let ctx = self.context_for_next_block(parent, attributes)?;
        Ok(self.create_block_builder::<DB>(evm, evm_env, parent, ctx, hashed_state_mode))
    }

    /// Creates an EVM instance for single-transaction execution with an inspector.
    #[cfg(feature = "std")]
    fn evm_with_env_and_inspector<DB, I>(
        &self,
        db: DB,
        evm_env: EvmEnvFor<Self>,
        inspector: I,
    ) -> evm2::Evm<evm2::BaseEvmTypes>
    where
        DB: evm2::evm::DynDatabase + 'static,
        I: evm2::Inspector<evm2::BaseEvmTypes> + 'static,
    {
        let mut evm = self.evm_with_env(db, evm_env);
        evm.set_inspector(inspector);
        evm
    }

    /// Applies block-level state changes required before transaction execution.
    #[cfg(feature = "std")]
    fn pre_block_state_changes<'a, DB>(
        &self,
        db: DB,
        evm_env: EvmEnvFor<Self>,
        block_number: u64,
        ctx: ExecutionCtxFor<'a, Self>,
    ) -> Result<evm2::BlockStateAccumulator, Box<dyn Error + Send + Sync>>
    where
        Self: 'a,
        DB: evm2::evm::Database + 'static,
        DB::Error: Error + Send + Sync + 'static;
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
