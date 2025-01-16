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

use alloy_consensus::BlockHeader as _;
use alloy_primitives::{Address, Bytes, B256, U256};
use reth_primitives_traits::{BlockHeader, SignedTransaction};
use revm::{Database, DatabaseCommit, GetInspector};
use revm_primitives::{
    BlockEnv, CfgEnvWithHandlerCfg, EVMError, Env, ResultAndState, SpecId, TxEnv, TxKind,
};

pub mod either;
/// EVM environment configuration.
pub mod env;
pub mod execute;
pub use env::EvmEnv;

#[cfg(feature = "std")]
pub mod metrics;
pub mod noop;
pub mod state_change;
pub mod system_calls;
#[cfg(any(test, feature = "test-utils"))]
/// test helpers for mocking executor
pub mod test_utils;

/// An abstraction over EVM.
pub trait Evm {
    /// Database type held by the EVM.
    type DB;
    /// Transaction environment
    type Tx;
    /// Error type.
    type Error;

    /// Reference to [`BlockEnv`].
    fn block(&self) -> &BlockEnv;

    /// Consumes the type and returns the underlying [`EvmEnv`].
    fn into_env(self) -> EvmEnv;

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

impl<EXT, DB> Evm for revm::Evm<'_, EXT, DB>
where
    DB: Database,
{
    type DB = DB;
    type Tx = TxEnv;
    type Error = EVMError<DB::Error>;

    fn block(&self) -> &BlockEnv {
        self.block()
    }

    fn into_env(self) -> EvmEnv {
        let Env { cfg, block, tx: _ } = *self.context.evm.inner.env;
        EvmEnv {
            cfg_env_with_handler_cfg: CfgEnvWithHandlerCfg {
                cfg_env: cfg,
                handler_cfg: self.handler.cfg,
            },
            block_env: block,
        }
    }

    fn transact(&mut self, tx: Self::Tx) -> Result<ResultAndState, Self::Error> {
        *self.tx_mut() = tx;
        self.transact()
    }

    fn transact_system_call(
        &mut self,
        caller: Address,
        contract: Address,
        data: Bytes,
    ) -> Result<ResultAndState, Self::Error> {
        #[allow(clippy::needless_update)] // side-effect of optimism fields
        let tx_env = TxEnv {
            caller,
            transact_to: TxKind::Call(contract),
            // Explicitly set nonce to None so revm does not do any nonce checks
            nonce: None,
            gas_limit: 30_000_000,
            value: U256::ZERO,
            data,
            // Setting the gas price to zero enforces that no value is transferred as part of the
            // call, and that the call will not count against the block's gas limit
            gas_price: U256::ZERO,
            // The chain ID check is not relevant here and is disabled if set to None
            chain_id: None,
            // Setting the gas priority fee to None ensures the effective gas price is derived from
            // the `gas_price` field, which we need to be zero
            gas_priority_fee: None,
            access_list: Vec::new(),
            // blob fields can be None for this tx
            blob_hashes: Vec::new(),
            max_fee_per_blob_gas: None,
            // TODO remove this once this crate is no longer built with optimism
            ..Default::default()
        };

        *self.tx_mut() = tx_env;

        let prev_block_env = self.block().clone();

        // ensure the block gas limit is >= the tx
        self.block_mut().gas_limit = U256::from(self.tx().gas_limit);

        // disable the base fee check for this call by setting the base fee to zero
        self.block_mut().basefee = U256::ZERO;

        let res = self.transact();

        // re-set the block env
        *self.block_mut() = prev_block_env;

        res
    }

    fn db_mut(&mut self) -> &mut Self::DB {
        &mut self.context.evm.db
    }
}

/// Trait for configuring the EVM for executing full blocks.
pub trait ConfigureEvm: ConfigureEvmEnv {
    /// Returns a new EVM with the given database configured with the given environment settings,
    /// including the spec id and transaction environment.
    ///
    /// This will preserve any handler modifications
    fn evm_with_env<'a, DB: Database + 'a>(
        &self,
        db: DB,
        evm_env: EvmEnv,
        tx: TxEnv,
    ) -> impl Evm<Tx = TxEnv, DB = DB, Error = EVMError<DB::Error>> + 'a;

    /// Returns a new EVM with the given database configured with `cfg` and `block_env`
    /// configuration derived from the given header. Relies on
    /// [`ConfigureEvmEnv::cfg_and_block_env`].
    ///
    /// # Caution
    ///
    /// This does not initialize the tx environment.
    fn evm_for_block<'a, DB: Database + 'a>(
        &self,
        db: DB,
        header: &Self::Header,
    ) -> impl Evm<Tx = TxEnv, DB = DB, Error = EVMError<DB::Error>> + 'a {
        let evm_env = self.cfg_and_block_env(header);
        self.evm_with_env(db, evm_env, Default::default())
    }

    /// Returns a new EVM with the given database configured with the given environment settings,
    /// including the spec id.
    ///
    /// This will use the given external inspector as the EVM external context.
    ///
    /// This will preserve any handler modifications
    fn evm_with_env_and_inspector<'a, DB, I>(
        &self,
        db: DB,
        evm_env: EvmEnv,
        tx: TxEnv,
        inspector: I,
    ) -> impl Evm<Tx = TxEnv, DB = DB, Error = EVMError<DB::Error>> + 'a
    where
        DB: Database + 'a,
        I: GetInspector<DB> + 'a;
}

impl<'b, T> ConfigureEvm for &'b T
where
    T: ConfigureEvm,
    &'b T: ConfigureEvmEnv<Header = T::Header>,
{
    fn evm_for_block<'a, DB: Database + 'a>(
        &self,
        db: DB,
        header: &Self::Header,
    ) -> impl Evm<Tx = TxEnv, DB = DB, Error = EVMError<DB::Error>> + 'a {
        (*self).evm_for_block(db, header)
    }

    fn evm_with_env<'a, DB: Database + 'a>(
        &self,
        db: DB,
        evm_env: EvmEnv,
        tx: TxEnv,
    ) -> impl Evm<Tx = TxEnv, DB = DB, Error = EVMError<DB::Error>> + 'a {
        (*self).evm_with_env(db, evm_env, tx)
    }

    fn evm_with_env_and_inspector<'a, DB, I>(
        &self,
        db: DB,
        evm_env: EvmEnv,
        tx_env: TxEnv,
        inspector: I,
    ) -> impl Evm<Tx = TxEnv, DB = DB, Error = EVMError<DB::Error>> + 'a
    where
        DB: Database + 'a,
        I: GetInspector<DB> + 'a,
    {
        (*self).evm_with_env_and_inspector(db, evm_env, tx_env, inspector)
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

    /// The error type that is returned by [`Self::next_cfg_and_block_env`].
    type Error: core::error::Error + Send + Sync;

    /// Returns a [`TxEnv`] from a transaction and [`Address`].
    fn tx_env(&self, transaction: &Self::Transaction, signer: Address) -> TxEnv {
        let mut tx_env = TxEnv::default();
        self.fill_tx_env(&mut tx_env, transaction, signer);
        tx_env
    }

    /// Fill transaction environment from a transaction  and the given sender address.
    fn fill_tx_env(&self, tx_env: &mut TxEnv, transaction: &Self::Transaction, sender: Address);

    /// Fill transaction environment with a system contract call.
    fn fill_tx_env_system_contract_call(
        &self,
        env: &mut Env,
        caller: Address,
        contract: Address,
        data: Bytes,
    );

    /// Returns a [`CfgEnvWithHandlerCfg`] for the given header.
    fn cfg_env(&self, header: &Self::Header) -> CfgEnvWithHandlerCfg {
        let mut cfg = CfgEnvWithHandlerCfg::new(Default::default(), Default::default());
        self.fill_cfg_env(&mut cfg, header);
        cfg
    }

    /// Fill [`CfgEnvWithHandlerCfg`] fields according to the chain spec and given header.
    ///
    /// This __must__ set the corresponding spec id in the handler cfg, based on timestamp or total
    /// difficulty
    fn fill_cfg_env(&self, cfg_env: &mut CfgEnvWithHandlerCfg, header: &Self::Header);

    /// Fill [`BlockEnv`] field according to the chain spec and given header
    fn fill_block_env(&self, block_env: &mut BlockEnv, header: &Self::Header, spec_id: SpecId) {
        block_env.number = U256::from(header.number());
        block_env.coinbase = header.beneficiary();
        block_env.timestamp = U256::from(header.timestamp());
        if spec_id >= SpecId::MERGE {
            block_env.prevrandao = header.mix_hash();
            block_env.difficulty = U256::ZERO;
        } else {
            block_env.difficulty = header.difficulty();
            block_env.prevrandao = None;
        }
        block_env.basefee = U256::from(header.base_fee_per_gas().unwrap_or_default());
        block_env.gas_limit = U256::from(header.gas_limit());

        // EIP-4844 excess blob gas of this block, introduced in Cancun
        if let Some(excess_blob_gas) = header.excess_blob_gas() {
            block_env.set_blob_excess_gas_and_price(excess_blob_gas, spec_id >= SpecId::PRAGUE);
        }
    }

    /// Creates a new [`EvmEnv`] for the given header.
    fn cfg_and_block_env(&self, header: &Self::Header) -> EvmEnv {
        let mut cfg = CfgEnvWithHandlerCfg::new(Default::default(), Default::default());
        let mut block_env = BlockEnv::default();
        self.fill_cfg_and_block_env(&mut cfg, &mut block_env, header);
        EvmEnv::new(cfg, block_env)
    }

    /// Convenience function to call both [`fill_cfg_env`](ConfigureEvmEnv::fill_cfg_env) and
    /// [`ConfigureEvmEnv::fill_block_env`].
    ///
    /// Note: Implementers should ensure that all fields are required fields are filled.
    fn fill_cfg_and_block_env(
        &self,
        cfg: &mut CfgEnvWithHandlerCfg,
        block_env: &mut BlockEnv,
        header: &Self::Header,
    ) {
        self.fill_cfg_env(cfg, header);
        self.fill_block_env(block_env, header, cfg.handler_cfg.spec_id);
    }

    /// Returns the configured [`EvmEnv`] for `parent + 1` block.
    ///
    /// This is intended for usage in block building after the merge and requires additional
    /// attributes that can't be derived from the parent block: attributes that are determined by
    /// the CL, such as the timestamp, suggested fee recipient, and randomness value.
    fn next_cfg_and_block_env(
        &self,
        parent: &Self::Header,
        attributes: NextBlockEnvAttributes,
    ) -> Result<EvmEnv, Self::Error>;
}

/// Represents additional attributes required to configure the next block.
/// This is used to configure the next block's environment
/// [`ConfigureEvmEnv::next_cfg_and_block_env`] and contains fields that can't be derived from the
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

/// Function hook that allows to modify a transaction environment.
pub trait TxEnvOverrides {
    /// Apply the overrides by modifying the given `TxEnv`.
    fn apply(&mut self, env: &mut TxEnv);
}

impl<F> TxEnvOverrides for F
where
    F: FnMut(&mut TxEnv),
{
    fn apply(&mut self, env: &mut TxEnv) {
        self(env)
    }
}
