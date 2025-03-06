//! Traits for configuring an EVM specifics.
//!
//! # Revm features
//!
//! This crate does __not__ enforce specific revm features such as `blst` or `c-kzg`, which are
//! critical for revm's evm internals, it is the responsibility of the implementer to ensure the
//! proper features are selected.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloy_eips::{eip2930::AccessList, eip4895::Withdrawals};
pub use alloy_evm::evm::EvmFactory;
use alloy_evm::IntoTxEnv;
use alloy_primitives::{Address, B256};
use core::fmt::Debug;
use reth_primitives_traits::{BlockHeader, SignedTransaction};
use revm::{context::TxEnv, inspector::Inspector};

pub mod either;
/// EVM environment configuration.
pub mod execute;

mod aliases;
pub use aliases::*;

#[cfg(feature = "metrics")]
pub mod metrics;
pub mod noop;
pub mod state_change;
pub mod system_calls;
#[cfg(any(test, feature = "test-utils"))]
/// test helpers for mocking executor
pub mod test_utils;

pub use alloy_evm::{Database, Evm, EvmEnv, EvmError, FromRecoveredTx, InvalidTxError};

/// Alias for `EvmEnv<<Evm as ConfigureEvmEnv>::Spec>`
pub type EvmEnvFor<Evm> = EvmEnv<<Evm as ConfigureEvmEnv>::Spec>;

/// Helper trait to bound [`Inspector`] for a [`ConfigureEvm`].
pub trait InspectorFor<DB: Database, Evm: ConfigureEvm>:
    Inspector<<Evm::EvmFactory as EvmFactory>::Context<DB>>
{
}
impl<T, DB, Evm> InspectorFor<DB, Evm> for T
where
    DB: Database,
    Evm: ConfigureEvm,
    T: Inspector<<Evm::EvmFactory as EvmFactory>::Context<DB>>,
{
}

/// Trait for configuring the EVM for executing full blocks.
pub trait ConfigureEvm: ConfigureEvmEnv {
    /// The EVM factory.
    type EvmFactory: EvmFactory<Tx = Self::TxEnv, Spec = Self::Spec>;

    /// Provides a reference to [`EvmFactory`] implementation.
    fn evm_factory(&self) -> &Self::EvmFactory;

    /// Returns a new EVM with the given database configured with the given environment settings,
    /// including the spec id and transaction environment.
    ///
    /// This will preserve any handler modifications
    fn evm_with_env<DB: Database>(&self, db: DB, evm_env: EvmEnv<Self::Spec>) -> EvmFor<Self, DB> {
        self.evm_factory().create_evm(db, evm_env)
    }

    /// Returns a new EVM with the given database configured with `cfg` and `block_env`
    /// configuration derived from the given header. Relies on
    /// [`ConfigureEvmEnv::evm_env`].
    ///
    /// # Caution
    ///
    /// This does not initialize the tx environment.
    fn evm_for_block<DB: Database>(&self, db: DB, header: &Self::Header) -> EvmFor<Self, DB> {
        let evm_env = self.evm_env(header);
        self.evm_with_env(db, evm_env)
    }

    /// Returns a new EVM with the given database configured with the given environment settings,
    /// including the spec id.
    ///
    /// This will use the given external inspector as the EVM external context.
    ///
    /// This will preserve any handler modifications
    fn evm_with_env_and_inspector<DB, I>(
        &self,
        db: DB,
        evm_env: EvmEnv<Self::Spec>,
        inspector: I,
    ) -> EvmFor<Self, DB, I>
    where
        DB: Database,
        I: InspectorFor<DB, Self>,
    {
        self.evm_factory().create_evm_with_inspector(db, evm_env, inspector)
    }
}

impl<'b, T> ConfigureEvm for &'b T
where
    T: ConfigureEvm,
    &'b T: ConfigureEvmEnv<Header = T::Header, TxEnv = T::TxEnv, Spec = T::Spec>,
{
    type EvmFactory = T::EvmFactory;

    fn evm_factory(&self) -> &Self::EvmFactory {
        (*self).evm_factory()
    }

    fn evm_for_block<DB: Database>(&self, db: DB, header: &Self::Header) -> EvmFor<Self, DB> {
        (*self).evm_for_block(db, header)
    }

    fn evm_with_env<DB: Database>(&self, db: DB, evm_env: EvmEnv<Self::Spec>) -> EvmFor<Self, DB> {
        (*self).evm_with_env(db, evm_env)
    }

    fn evm_with_env_and_inspector<DB, I>(
        &self,
        db: DB,
        evm_env: EvmEnv<Self::Spec>,
        inspector: I,
    ) -> EvmFor<Self, DB, I>
    where
        DB: Database,
        I: InspectorFor<DB, Self>,
    {
        (*self).evm_with_env_and_inspector(db, evm_env, inspector)
    }
}

/// This represents the set of methods used to configure the EVM's environment before block
/// execution.
///
/// Default trait method  implementation is done w.r.t. L1.
#[auto_impl::auto_impl(&, Arc)]
pub trait ConfigureEvmEnv: Send + Sync + Unpin + Clone {
    /// The header type used by the EVM.
    type Header: BlockHeader;

    /// The transaction type.
    type Transaction: SignedTransaction;

    /// Transaction environment used by EVM.
    type TxEnv: TransactionEnv + FromRecoveredTx<Self::Transaction> + IntoTxEnv<Self::TxEnv>;

    /// The error type that is returned by [`Self::next_evm_env`].
    type Error: core::error::Error + Send + Sync + 'static;

    /// Identifier of the EVM specification.
    type Spec: Debug + Copy + Send + Sync + 'static;

    /// Context required for configuring next block environment.
    ///
    /// Contains values that can't be derived from the parent block.
    type NextBlockEnvCtx: Debug + Clone;

    /// Returns a [`TxEnv`] from a transaction and [`Address`].
    fn tx_env(&self, transaction: impl IntoTxEnv<Self::TxEnv>) -> Self::TxEnv {
        transaction.into_tx_env()
    }

    /// Creates a new [`EvmEnv`] for the given header.
    fn evm_env(&self, header: &Self::Header) -> EvmEnv<Self::Spec>;

    /// Returns the configured [`EvmEnv`] for `parent + 1` block.
    ///
    /// This is intended for usage in block building after the merge and requires additional
    /// attributes that can't be derived from the parent block: attributes that are determined by
    /// the CL, such as the timestamp, suggested fee recipient, and randomness value.
    fn next_evm_env(
        &self,
        parent: &Self::Header,
        attributes: &Self::NextBlockEnvCtx,
    ) -> Result<EvmEnv<Self::Spec>, Self::Error>;
}

/// Represents additional attributes required to configure the next block.
/// This is used to configure the next block's environment
/// [`ConfigureEvmEnv::next_evm_env`] and contains fields that can't be derived from the
/// parent header alone (attributes that are determined by the CL.)
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
}

/// Abstraction over transaction environment.
pub trait TransactionEnv:
    revm::context_interface::Transaction + Debug + Clone + Send + Sync + 'static
{
    /// Set the gas limit.
    fn set_gas_limit(&mut self, gas_limit: u64);

    /// Set the gas limit.
    fn with_gas_limit(mut self, gas_limit: u64) -> Self {
        self.set_gas_limit(gas_limit);
        self
    }

    /// Returns the configured nonce.
    fn nonce(&self) -> u64;

    /// Sets the nonce.
    fn set_nonce(&mut self, nonce: u64);

    /// Sets the nonce.
    fn with_nonce(mut self, nonce: u64) -> Self {
        self.set_nonce(nonce);
        self
    }

    /// Set access list.
    fn set_access_list(&mut self, access_list: AccessList);

    /// Set access list.
    fn with_access_list(mut self, access_list: AccessList) -> Self {
        self.set_access_list(access_list);
        self
    }
}

impl TransactionEnv for TxEnv {
    fn set_gas_limit(&mut self, gas_limit: u64) {
        self.gas_limit = gas_limit;
    }

    fn nonce(&self) -> u64 {
        self.nonce
    }

    fn set_nonce(&mut self, nonce: u64) {
        self.nonce = nonce;
    }

    fn set_access_list(&mut self, access_list: AccessList) {
        self.access_list = access_list;
    }
}

#[cfg(feature = "op")]
impl<T: TransactionEnv> TransactionEnv for op_revm::OpTransaction<T> {
    fn set_gas_limit(&mut self, gas_limit: u64) {
        self.base.set_gas_limit(gas_limit);
    }

    fn nonce(&self) -> u64 {
        TransactionEnv::nonce(&self.base)
    }

    fn set_nonce(&mut self, nonce: u64) {
        self.base.set_nonce(nonce);
    }

    fn set_access_list(&mut self, access_list: AccessList) {
        self.base.set_access_list(access_list);
    }
}
