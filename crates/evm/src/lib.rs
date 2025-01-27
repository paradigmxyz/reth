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

use alloy_consensus::transaction::Recovered;
use alloy_eips::eip2930::AccessList;
use alloy_primitives::{Address, Bytes, B256, U256};
use core::fmt::Debug;
use reth_primitives_traits::{BlockHeader, SignedTransaction};
use revm::{Database, DatabaseCommit, GetInspector};
use revm_primitives::{BlockEnv, EVMError, ResultAndState, TxEnv, TxKind};

pub mod either;
/// EVM environment configuration.
pub mod env;
pub mod execute;
pub use env::EvmEnv;

mod aliases;
pub use aliases::*;

#[cfg(feature = "std")]
pub mod metrics;
pub mod noop;
pub mod state_change;
pub mod system_calls;
#[cfg(any(test, feature = "test-utils"))]
/// test helpers for mocking executor
pub mod test_utils;

/// An abstraction over EVM.
///
/// At this point, assumed to be implemented on wrappers around [`revm::Evm`].
pub trait Evm {
    /// Database type held by the EVM.
    type DB;
    /// Transaction environment
    type Tx;
    /// Error type.
    type Error;

    /// Reference to [`BlockEnv`].
    fn block(&self) -> &BlockEnv;

    /// Executes the given transaction.
    fn transact(&mut self, tx: Self::Tx) -> Result<ResultAndState, Self::Error>;

    /// Executes a system call.
    fn transact_system_call(
        &mut self,
        caller: Address,
        contract: Address,
        data: Bytes,
    ) -> Result<ResultAndState, Self::Error>;

    /// Returns a mutable reference to the underlying database.
    fn db_mut(&mut self) -> &mut Self::DB;

    /// Executes a transaction and commits the state changes to the underlying database.
    fn transact_commit(&mut self, tx_env: Self::Tx) -> Result<ResultAndState, Self::Error>
    where
        Self::DB: DatabaseCommit,
    {
        let result = self.transact(tx_env)?;
        self.db_mut().commit(result.state.clone());

        Ok(result)
    }
}

/// Trait for configuring the EVM for executing full blocks.
pub trait ConfigureEvm: ConfigureEvmEnv {
    /// The EVM implementation.
    type Evm<'a, DB: Database + 'a, I: 'a>: Evm<
        Tx = Self::TxEnv,
        DB = DB,
        Error = EVMError<DB::Error>,
    >;

    /// Returns a new EVM with the given database configured with the given environment settings,
    /// including the spec id and transaction environment.
    ///
    /// This will preserve any handler modifications
    fn evm_with_env<DB: Database>(
        &self,
        db: DB,
        evm_env: EvmEnv<Self::Spec>,
    ) -> Self::Evm<'_, DB, ()>;

    /// Returns a new EVM with the given database configured with `cfg` and `block_env`
    /// configuration derived from the given header. Relies on
    /// [`ConfigureEvmEnv::evm_env`].
    ///
    /// # Caution
    ///
    /// This does not initialize the tx environment.
    fn evm_for_block<DB: Database>(&self, db: DB, header: &Self::Header) -> Self::Evm<'_, DB, ()> {
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
    ) -> Self::Evm<'_, DB, I>
    where
        DB: Database,
        I: GetInspector<DB>;
}

impl<'b, T> ConfigureEvm for &'b T
where
    T: ConfigureEvm,
    &'b T: ConfigureEvmEnv<Header = T::Header, TxEnv = T::TxEnv, Spec = T::Spec>,
{
    type Evm<'a, DB: Database + 'a, I: 'a> = T::Evm<'a, DB, I>;

    fn evm_for_block<DB: Database>(&self, db: DB, header: &Self::Header) -> Self::Evm<'_, DB, ()> {
        (*self).evm_for_block(db, header)
    }

    fn evm_with_env<DB: Database>(
        &self,
        db: DB,
        evm_env: EvmEnv<Self::Spec>,
    ) -> Self::Evm<'_, DB, ()> {
        (*self).evm_with_env(db, evm_env)
    }

    fn evm_with_env_and_inspector<DB, I>(
        &self,
        db: DB,
        evm_env: EvmEnv<Self::Spec>,
        inspector: I,
    ) -> Self::Evm<'_, DB, I>
    where
        DB: Database,
        I: GetInspector<DB>,
    {
        (*self).evm_with_env_and_inspector(db, evm_env, inspector)
    }
}

/// This represents the set of methods used to configure the EVM's environment before block
/// execution.
///
/// Default trait method  implementation is done w.r.t. L1.
#[auto_impl::auto_impl(&, Arc)]
pub trait ConfigureEvmEnv: Send + Sync + Unpin + Clone + 'static {
    /// The header type used by the EVM.
    type Header: BlockHeader;

    /// The transaction type.
    type Transaction: SignedTransaction;

    /// Transaction environment used by EVM.
    type TxEnv: TransactionEnv;

    /// The error type that is returned by [`Self::next_evm_env`].
    type Error: core::error::Error + Send + Sync;

    /// Identifier of the EVM specification.
    type Spec: Into<revm_primitives::SpecId> + Debug + Copy + Send + Sync;

    /// Returns a [`TxEnv`] from a transaction and [`Address`].
    fn tx_env(&self, transaction: &Self::Transaction, signer: Address) -> Self::TxEnv;

    /// Returns a [`TxEnv`] from a [`Recovered`] transaction.
    fn tx_env_from_recovered(&self, tx: Recovered<&Self::Transaction>) -> Self::TxEnv {
        let (tx, address) = tx.into_parts();
        self.tx_env(tx, address)
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
        attributes: NextBlockEnvAttributes,
    ) -> Result<EvmEnv<Self::Spec>, Self::Error>;
}

/// Represents additional attributes required to configure the next block.
/// This is used to configure the next block's environment
/// [`ConfigureEvmEnv::next_evm_env`] and contains fields that can't be derived from the
/// parent header alone (attributes that are determined by the CL.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NextBlockEnvAttributes {
    /// The timestamp of the next block.
    pub timestamp: u64,
    /// The suggested fee recipient for the next block.
    pub suggested_fee_recipient: Address,
    /// The randomness value for the next block.
    pub prev_randao: B256,
    /// Block gas limit.
    pub gas_limit: u64,
}

/// Abstraction over transaction environment.
pub trait TransactionEnv:
    Into<revm_primitives::TxEnv> + Debug + Default + Clone + Send + Sync + 'static
{
    /// Returns configured gas limit.
    fn gas_limit(&self) -> u64;

    /// Set the gas limit.
    fn set_gas_limit(&mut self, gas_limit: u64);

    /// Set the gas limit.
    fn with_gas_limit(mut self, gas_limit: u64) -> Self {
        self.set_gas_limit(gas_limit);
        self
    }

    /// Returns configured gas price.
    fn gas_price(&self) -> U256;

    /// Returns configured value.
    fn value(&self) -> U256;

    /// Caller of the transaction.
    fn caller(&self) -> Address;

    /// Set access list.
    fn set_access_list(&mut self, access_list: AccessList);

    /// Set access list.
    fn with_access_list(mut self, access_list: AccessList) -> Self {
        self.set_access_list(access_list);
        self
    }

    /// Returns calldata for the transaction.
    fn input(&self) -> &Bytes;

    /// Returns [`TxKind`] of the transaction.
    fn kind(&self) -> TxKind;
}

impl TransactionEnv for TxEnv {
    fn gas_limit(&self) -> u64 {
        self.gas_limit
    }

    fn set_gas_limit(&mut self, gas_limit: u64) {
        self.gas_limit = gas_limit;
    }

    fn gas_price(&self) -> U256 {
        self.gas_price.to()
    }

    fn value(&self) -> U256 {
        self.value
    }

    fn caller(&self) -> Address {
        self.caller
    }

    fn set_access_list(&mut self, access_list: AccessList) {
        self.access_list = access_list.to_vec();
    }

    fn input(&self) -> &Bytes {
        &self.data
    }

    fn kind(&self) -> TxKind {
        self.transact_to
    }
}
