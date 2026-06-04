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

use crate::execute::{IntoTxEnv, UnsupportedExecutor};
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

mod aliases;
pub use aliases::*;

#[cfg(feature = "std")]
mod engine;
#[cfg(feature = "std")]
pub use engine::{
    ConfigureEngineEvm, ConfigureEvm2BlockExecutor, ConfigureEvm2Engine, ConvertTx,
    ExecutableTxIterator, ExecutableTxTuple,
};

#[cfg(feature = "metrics")]
pub mod metrics;
pub mod noop;
#[cfg(any(test, feature = "test-utils"))]
/// test helpers for mocking executor
pub mod test_utils;

/// A complete configuration of EVM for Reth.
///
/// The active execution path is evm2-native. The old legacy executor block executor and builder
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

    /// Returns a parked executor for old legacy executor block execution call sites.
    #[auto_impl(keep_default_for(&, Arc))]
    fn executor<DB>(&self, _db: DB) -> UnsupportedExecutor<Self::Primitives> {
        UnsupportedExecutor::default()
    }

    /// Returns a parked batch executor for old legacy executor block execution call sites.
    #[auto_impl(keep_default_for(&, Arc))]
    fn batch_executor<DB>(&self, _db: DB) -> UnsupportedExecutor<Self::Primitives> {
        UnsupportedExecutor::default()
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
    /// Withdrawals.
    pub withdrawals: Option<Withdrawals>,
    /// Optional extra data.
    pub extra_data: Bytes,
    /// Optional slot number for post-Amsterdam payloads.
    pub slot_number: Option<u64>,
}
