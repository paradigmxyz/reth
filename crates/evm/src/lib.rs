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

use crate::execute::BasicBlockBuilder;
use alloc::vec::Vec;
use alloy_eips::{eip2930::AccessList, eip4895::Withdrawals};
use alloy_evm::block::{BlockExecutorFactory, BlockExecutorFor};
use alloy_primitives::{Address, B256};
use core::{error::Error, fmt::Debug};
use execute::{BlockAssembler, BlockBuilder};
use reth_primitives_traits::{
    BlockTy, HeaderTy, NodePrimitives, ReceiptTy, SealedBlock, SealedHeader, TxTy,
};
use revm::context::TxEnv;
use revm_database::State;

pub mod either;
/// EVM environment configuration.
pub mod execute;

mod aliases;
pub use aliases::*;

#[cfg(feature = "metrics")]
pub mod metrics;
pub mod noop;
#[cfg(any(test, feature = "test-utils"))]
/// test helpers for mocking executor
pub mod test_utils;

pub use alloy_evm::{
    block::{state_changes, system_calls, OnStateHook},
    *,
};

pub use alloy_evm::block::state_changes as state_change;

/// A complete configuration of EVM for Reth.
///
/// This trait encapsulates complete configuration required for transaction execution and block
/// execution/building.
///
/// The EVM abstraction consists of the following layers:
///     - [`Evm`] produced by [`EvmFactory`]: The EVM implementation responsilble for executing
///       individual transactions and producing output for them including state changes, logs, gas
///       usage, etc.
///     - [`BlockExecutor`] produced by [`BlockExecutorFactory`]: Executor operates on top of
///       [`Evm`] and is responsible for executing entire blocks. This is different from simply
///       aggregating outputs of transactions execution as it also involves higher level state
///       changes such as receipt building, applying block rewards, system calls, etc.
///     - [`BlockAssembler`]: Encapsulates logic for assembling blocks. It operates on context and
///       output of [`BlockExecutor`], and is required to know how to assemble a next block to
///       include in the chain.
///
/// All of the above components need configuration environment which we are abstracting over to
/// allow plugging EVM implementation into Reth SDK.
///
/// The abstraction is designed to serve 2 codepaths:
///     1. Externally provided complete block (e.g received while syncing).
///     2. Block building when we know parent block and some additional context obtained from
///       payload attributes or alike.
///
/// First case is handled by [`ConfigureEvm::evm_env`] and [`ConfigureEvm::context_for_block`]
/// which implement a conversion from [`NodePrimitives::Block`] to [`EvmEnv`] and [`ExecutionCtx`],
/// and allow configuring EVM and block execution environment at a given block.
///
/// Second case is handled by similar [`ConfigureEvm::next_evm_env`] and
/// [`ConfigureEvm::context_for_next_block`] which take parent [`NodePrimitives::BlockHeader`]
/// along with [`NextBlockEnvCtx`]. [`NextBlockEnvCtx`] is very similar to payload attributes and
/// simply contains context for next block that is generally received from a CL node (timestamp,
/// beneficiary, withdrawals, etc.).
///
/// [`ExecutionCtx`]: BlockExecutorFactory::ExecutionCtx
/// [`NextBlockEnvCtx`]: ConfigureEvm::NextBlockEnvCtx
/// [`BlockExecutor`]: alloy_evm::block::BlockExecutor
#[auto_impl::auto_impl(&, Arc)]
pub trait ConfigureEvm: Send + Sync + Unpin + Clone {
    /// The primitives type used by the EVM.
    type Primitives: NodePrimitives;

    /// The error type that is returned by [`Self::next_evm_env`].
    type Error: Error + Send + Sync + 'static;

    /// Context required for configuring next block environment.
    ///
    /// Contains values that can't be derived from the parent block.
    type NextBlockEnvCtx: Debug + Clone;

    /// Configured [`BlockExecutorFactory`], contains [`EvmFactory`] internally.
    type BlockExecutorFactory: BlockExecutorFactory<
        Transaction = TxTy<Self::Primitives>,
        Receipt = ReceiptTy<Self::Primitives>,
        EvmFactory: EvmFactory<Tx: TransactionEnv + FromRecoveredTx<TxTy<Self::Primitives>>>,
    >;

    /// A type that knows how to build a block.
    type BlockAssembler: BlockAssembler<
        Self::BlockExecutorFactory,
        Block = BlockTy<Self::Primitives>,
    >;

    /// Returns reference to the configured [`BlockExecutorFactory`].
    fn block_executor_factory(&self) -> &Self::BlockExecutorFactory;

    /// Returns reference to the configured [`BlockAssembler`].
    fn block_assembler(&self) -> &Self::BlockAssembler;

    /// Creates a new [`EvmEnv`] for the given header.
    fn evm_env(&self, header: &HeaderTy<Self::Primitives>) -> EvmEnvFor<Self>;

    /// Returns the configured [`EvmEnv`] for `parent + 1` block.
    ///
    /// This is intended for usage in block building after the merge and requires additional
    /// attributes that can't be derived from the parent block: attributes that are determined by
    /// the CL, such as the timestamp, suggested fee recipient, and randomness value.
    fn next_evm_env(
        &self,
        parent: &HeaderTy<Self::Primitives>,
        attributes: &Self::NextBlockEnvCtx,
    ) -> Result<EvmEnvFor<Self>, Self::Error>;

    /// Returns the configured [`BlockExecutorFactory::ExecutionCtx`] for a given block.
    fn context_for_block<'a>(
        &self,
        block: &'a SealedBlock<BlockTy<Self::Primitives>>,
    ) -> ExecutionCtxFor<'a, Self>;

    /// Returns the configured [`BlockExecutorFactory::ExecutionCtx`] for `parent + 1`
    /// block.
    fn context_for_next_block(
        &self,
        parent: &SealedHeader<HeaderTy<Self::Primitives>>,
        attributes: Self::NextBlockEnvCtx,
    ) -> ExecutionCtxFor<'_, Self>;

    /// Returns a [`TxEnv`] from a transaction and [`Address`].
    fn tx_env(&self, transaction: impl IntoTxEnv<TxEnvFor<Self>>) -> TxEnvFor<Self> {
        transaction.into_tx_env()
    }

    /// Provides a reference to [`EvmFactory`] implementation.
    fn evm_factory(&self) -> &EvmFactoryFor<Self> {
        self.block_executor_factory().evm_factory()
    }

    /// Returns a new EVM with the given database configured with the given environment settings,
    /// including the spec id and transaction environment.
    ///
    /// This will preserve any handler modifications
    fn evm_with_env<DB: Database>(&self, db: DB, evm_env: EvmEnvFor<Self>) -> EvmFor<Self, DB> {
        self.evm_factory().create_evm(db, evm_env)
    }

    /// Returns a new EVM with the given database configured with `cfg` and `block_env`
    /// configuration derived from the given header. Relies on
    /// [`ConfigureEvm::evm_env`].
    ///
    /// # Caution
    ///
    /// This does not initialize the tx environment.
    fn evm_for_block<DB: Database>(
        &self,
        db: DB,
        header: &HeaderTy<Self::Primitives>,
    ) -> EvmFor<Self, DB> {
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
        evm_env: EvmEnvFor<Self>,
        inspector: I,
    ) -> EvmFor<Self, DB, I>
    where
        DB: Database,
        I: InspectorFor<Self, DB>,
    {
        self.evm_factory().create_evm_with_inspector(db, evm_env, inspector)
    }

    /// Creates a strategy with given EVM and execution context.
    fn create_executor<'a, DB, I>(
        &'a self,
        evm: EvmFor<Self, &'a mut State<DB>, I>,
        ctx: <Self::BlockExecutorFactory as BlockExecutorFactory>::ExecutionCtx<'a>,
    ) -> impl BlockExecutorFor<'a, Self::BlockExecutorFactory, DB, I>
    where
        DB: Database,
        I: InspectorFor<Self, &'a mut State<DB>> + 'a,
    {
        self.block_executor_factory().create_executor(evm, ctx)
    }

    /// Creates a strategy for execution of a given block.
    fn executor_for_block<'a, DB: Database>(
        &'a self,
        db: &'a mut State<DB>,
        block: &'a SealedBlock<<Self::Primitives as NodePrimitives>::Block>,
    ) -> impl BlockExecutorFor<'a, Self::BlockExecutorFactory, DB> {
        let evm = self.evm_for_block(db, block.header());
        let ctx = self.context_for_block(block);
        self.create_executor(evm, ctx)
    }

    /// Creates a [`BlockBuilder`]. Should be used when building a new block.
    ///
    /// Block builder wraps an inner [`alloy_evm::block::BlockExecutor`] and has a similar
    /// interface. Builder collects all of the executed transactions, and once
    /// [`BlockBuilder::finish`] is called, it invokes the configured [`BlockAssembler`] to
    /// create a block.
    fn create_block_builder<'a, DB, I>(
        &'a self,
        evm: EvmFor<Self, &'a mut State<DB>, I>,
        parent: &'a SealedHeader<HeaderTy<Self::Primitives>>,
        ctx: <Self::BlockExecutorFactory as BlockExecutorFactory>::ExecutionCtx<'a>,
    ) -> impl BlockBuilder<
        Primitives = Self::Primitives,
        Executor: BlockExecutorFor<'a, Self::BlockExecutorFactory, DB, I>,
    >
    where
        DB: Database,
        I: InspectorFor<Self, &'a mut State<DB>> + 'a,
    {
        BasicBlockBuilder {
            executor: self.create_executor(evm, ctx.clone()),
            ctx,
            assembler: self.block_assembler(),
            parent,
            transactions: Vec::new(),
        }
    }

    /// Creates a [`BlockBuilder`] for building of a new block. This is a helper to invoke
    /// [`ConfigureEvm::create_block_builder`].
    fn builder_for_next_block<'a, DB: Database>(
        &'a self,
        db: &'a mut State<DB>,
        parent: &'a SealedHeader<<Self::Primitives as NodePrimitives>::BlockHeader>,
        attributes: Self::NextBlockEnvCtx,
    ) -> Result<impl BlockBuilder<Primitives = Self::Primitives>, Self::Error> {
        let evm_env = self.next_evm_env(parent, &attributes)?;
        let evm = self.evm_with_env(db, evm_env);
        let ctx = self.context_for_next_block(parent, attributes);
        Ok(self.create_block_builder(evm, parent, ctx))
    }
}

/// Represents additional attributes required to configure the next block.
/// This is used to configure the next block's environment
/// [`ConfigureEvm::next_evm_env`] and contains fields that can't be derived from the
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
