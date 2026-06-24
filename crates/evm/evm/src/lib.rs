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

use crate::execute::{BlockExecutor, Executor, HashedStateMode, IntoTxEnv};
use alloc::{boxed::Box, string::String};
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
    /// Returns the active EVM spec.
    fn spec_id(&self) -> evm2::SpecId;

    /// Returns the EVM block environment.
    fn block_env(&self) -> evm2::env::BlockEnv;
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
/// The active execution path is EVM-native. The old legacy executor block executor and builder
/// methods are intentionally parked behind stubs while BAL and payload building are ported.
#[auto_impl::auto_impl(&, Arc)]
pub trait ConfigureEvm: Clone + Debug + Send + Sync + Unpin {
    /// The primitives type used by the EVM.
    type Primitives: NodePrimitives;

    /// The error type that is returned by environment builders.
    type Error: Error + Send + Sync + 'static;

    /// Context required for configuring next block environment.
    type NextBlockEnvCtx: Debug + Clone;

    /// Configured EVM spec type.
    type Spec: Debug + Default + Clone + Send + Sync + 'static;

    /// Configured EVM environment type.
    type EvmEnv: Debug + Clone + Send + Sync + 'static;

    /// Configured transaction environment type.
    type TxEnv: From<Recovered<TxTy<Self::Primitives>>> + Clone + Send + Sync + 'static;

    /// Execution context for a block or payload.
    type ExecutionCtx<'a>: Debug + Clone + Send
    where
        Self: 'a;

    /// Executor returned for block execution over the provided database.
    type Executor<DB>: Executor<Primitives = Self::Primitives>
    where
        DB: evm2::evm::Database + Clone + 'static,
        DB::Error: core::error::Error + Send + Sync + 'static;

    /// Configured block executor returned for an active block execution.
    #[cfg(feature = "std")]
    type BlockExecutor<'a, DB>: BlockExecutor<
        Primitives = Self::Primitives,
        Transaction = TxEnvFor<Self>,
    >
    where
        Self: 'a,
        DB: evm2::evm::Database + Clone + 'static,
        DB::Error: core::error::Error + Send + Sync + 'static;

    /// Per-thread EVM instance used by prewarm workers.
    #[cfg(feature = "std")]
    type PrewarmEvm<DB>
    where
        DB: reth_storage_api::StateProvider + Send + 'static;

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
        db: DB,
        evm_env: EvmEnvFor<Self>,
        ctx: ExecutionCtxFor<'a, Self>,
        hashed_state_mode: HashedStateMode,
    ) -> Self::BlockExecutor<'a, DB>
    where
        Self: 'a,
        DB: evm2::evm::Database + Clone + 'static,
        DB::Error: core::error::Error + Send + Sync + 'static;

    /// Creates an EVM instance for single-transaction execution with the configured environment.
    #[cfg(feature = "std")]
    fn evm_with_env<DB>(&self, db: DB, evm_env: EvmEnvFor<Self>) -> evm2::Evm<evm2::BaseEvmTypes>
    where
        DB: evm2::evm::DynDatabase + 'static;

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

    /// Creates a prewarm evm over the provided state.
    #[cfg(feature = "std")]
    fn prewarm_evm<DB>(&self, state_provider: DB, env: EvmEnvFor<Self>) -> Self::PrewarmEvm<DB>
    where
        DB: reth_storage_api::StateProvider + Send + 'static;

    /// Executes a transaction for prewarming, streams its state changes into `sink`, and discards
    /// them.
    #[cfg(feature = "std")]
    fn prewarm_tx<DB, S>(
        &self,
        evm: &mut Self::PrewarmEvm<DB>,
        tx: TxEnvFor<Self>,
        sink: &mut S,
    ) -> Result<evm2::TxResult, Box<dyn core::error::Error + Send + Sync>>
    where
        DB: reth_storage_api::StateProvider + Send + 'static,
        S: evm2::evm::StateChangeSink<Error = core::convert::Infallible>;
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
