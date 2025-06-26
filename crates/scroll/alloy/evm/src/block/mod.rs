pub mod curie;
pub mod feynman;

pub use receipt_builder::{ReceiptBuilderCtx, ScrollReceiptBuilder};
mod receipt_builder;

use crate::{
    block::{
        curie::{apply_curie_hard_fork, L1_GAS_PRICE_ORACLE_ADDRESS},
        feynman::apply_feynman_hard_fork,
    },
    system_caller::ScrollSystemCaller,
    FromTxWithCompressionRatio, ScrollEvm, ScrollEvmFactory, ScrollTransactionIntoTxEnv,
    ToTxWithCompressionRatio,
};
use alloc::{boxed::Box, format, vec::Vec};

use alloy_consensus::{Transaction, TxReceipt, Typed2718};
use alloy_eips::Encodable2718;
use alloy_evm::{
    block::{
        BlockExecutionError, BlockExecutionResult, BlockExecutor, BlockExecutorFactory,
        BlockExecutorFor, BlockValidationError, CommitChanges, ExecutableTx, OnStateHook,
    },
    Database, Evm, EvmFactory, FromRecoveredTx, FromTxWithEncoded,
};
use alloy_primitives::{B256, U256};
use revm::{
    context::{
        result::{ExecutionResult, InvalidTransaction, ResultAndState},
        TxEnv,
    },
    database::State,
    handler::PrecompileProvider,
    interpreter::InterpreterResult,
    DatabaseCommit, Inspector,
};
use revm_scroll::builder::ScrollContext;
use scroll_alloy_consensus::L1_MESSAGE_TRANSACTION_TYPE;
use scroll_alloy_hardforks::{ScrollHardfork, ScrollHardforks};

/// A cache for transaction compression ratios.
pub type ScrollTxCompressionRatios = Vec<U256>;

/// Context for Scroll Block Execution.
#[derive(Debug, Default, Clone)]
pub struct ScrollBlockExecutionCtx {
    /// Parent block hash.
    pub parent_hash: B256,
}

/// Block executor for Scroll.
#[derive(Debug)]
pub struct ScrollBlockExecutor<Evm, R: ScrollReceiptBuilder, Spec> {
    /// Spec.
    spec: Spec,
    /// Receipt builder.
    receipt_builder: R,
    /// The EVM used by executor.
    evm: Evm,
    /// Context for block execution.
    ctx: ScrollBlockExecutionCtx,
    /// Receipts of executed transactions.
    receipts: Vec<R::Receipt>,
    /// Total gas used by executed transactions.
    gas_used: u64,
    /// Utility to call system smart contracts.
    system_caller: ScrollSystemCaller<Spec>,
}

impl<E, R: ScrollReceiptBuilder, Spec> ScrollBlockExecutor<E, R, Spec> {
    /// Returns the spec for [`ScrollBlockExecutor`].
    pub const fn spec(&self) -> &Spec {
        &self.spec
    }
}

impl<E, R, Spec> ScrollBlockExecutor<E, R, Spec>
where
    E: EvmExt,
    R: ScrollReceiptBuilder,
    Spec: ScrollHardforks + Clone,
{
    /// Creates a new [`ScrollBlockExecutor`].
    pub fn new(evm: E, ctx: ScrollBlockExecutionCtx, spec: Spec, receipt_builder: R) -> Self {
        Self {
            evm,
            ctx,
            system_caller: ScrollSystemCaller::new(spec.clone()),
            spec,
            receipt_builder,
            receipts: Vec::new(),
            gas_used: 0,
        }
    }
}

impl<'db, DB, E, R, Spec> ScrollBlockExecutor<E, R, Spec>
where
    DB: Database + 'db,
    E: EvmExt<
        DB = &'db mut State<DB>,
        Tx: FromRecoveredTx<R::Transaction>
                + FromTxWithEncoded<R::Transaction>
                + FromTxWithCompressionRatio<R::Transaction>,
    >,
    R: ScrollReceiptBuilder<Transaction: Transaction + Encodable2718, Receipt: TxReceipt>,
    Spec: ScrollHardforks,
{
    /// Executes all transactions in a block, applying pre and post execution changes. The provided
    /// transaction compression ratios are expected to be in the same order as the
    /// transactions.
    pub fn execute_block_with_compression_cache(
        mut self,
        transactions: impl IntoIterator<
            Item = impl ExecutableTx<Self>
                       + ToTxWithCompressionRatio<<Self as BlockExecutor>::Transaction>,
        >,
        compression_ratios: ScrollTxCompressionRatios,
    ) -> Result<BlockExecutionResult<R::Receipt>, BlockExecutionError>
    where
        Self: Sized,
    {
        self.apply_pre_execution_changes()?;

        for (tx, compression_ratio) in transactions.into_iter().zip(compression_ratios.into_iter())
        {
            let tx = tx.with_compression_ratio(compression_ratio);
            self.execute_transaction(&tx)?;
        }

        self.apply_post_execution_changes()
    }
}

impl<'db, DB, E, R, Spec> BlockExecutor for ScrollBlockExecutor<E, R, Spec>
where
    DB: Database + 'db,
    E: EvmExt<
        DB = &'db mut State<DB>,
        Tx: FromRecoveredTx<R::Transaction> + FromTxWithEncoded<R::Transaction>,
    >,
    R: ScrollReceiptBuilder<Transaction: Transaction + Encodable2718, Receipt: TxReceipt>,
    Spec: ScrollHardforks,
{
    type Transaction = R::Transaction;
    type Receipt = R::Receipt;
    type Evm = E;

    fn apply_pre_execution_changes(&mut self) -> Result<(), BlockExecutionError> {
        // set state clear flag if the block is after the Spurious Dragon hardfork.
        let state_clear_flag =
            self.spec.is_spurious_dragon_active_at_block(self.evm.block().number);
        self.evm.db_mut().set_state_clear_flag(state_clear_flag);

        // load the l1 gas oracle contract in cache.
        let _ = self
            .evm
            .db_mut()
            .load_cache_account(L1_GAS_PRICE_ORACLE_ADDRESS)
            .map_err(BlockExecutionError::other)?;

        // apply gas oracle predeploy upgrade at Curie transition block.
        if self
            .spec
            .scroll_fork_activation(ScrollHardfork::Curie)
            .transitions_at_block(self.evm.block().number)
        {
            if let Err(err) = apply_curie_hard_fork(self.evm.db_mut()) {
                return Err(BlockExecutionError::msg(format!(
                    "error occurred at Curie fork: {err:?}"
                )));
            };
        }

        // apply gas oracle predeploy upgrade at Feynman transition block.
        if self
            .spec
            .scroll_fork_activation(ScrollHardfork::Feynman)
            .active_at_timestamp(self.evm.block().timestamp)
        {
            if let Err(err) = apply_feynman_hard_fork(self.evm.db_mut()) {
                return Err(BlockExecutionError::msg(format!(
                    "error occurred at Feynman fork: {err:?}"
                )));
            };
        }

        // apply eip-2935.
        self.system_caller.apply_blockhashes_contract_call(self.ctx.parent_hash, &mut self.evm)?;

        Ok(())
    }

    fn execute_transaction_with_commit_condition(
        &mut self,
        tx: impl ExecutableTx<Self>,
        f: impl FnOnce(&ExecutionResult<<Self::Evm as Evm>::HaltReason>) -> CommitChanges,
    ) -> Result<Option<u64>, BlockExecutionError> {
        let chain_spec = &self.spec;
        let is_l1_message = tx.tx().ty() == L1_MESSAGE_TRANSACTION_TYPE;
        // The sum of the transaction’s gas limit and the gas utilized in this block prior,
        // must be no greater than the block’s gasLimit.
        let block_available_gas = self.evm.block().gas_limit - self.gas_used;
        if tx.tx().gas_limit() > block_available_gas {
            return Err(BlockValidationError::TransactionGasLimitMoreThanAvailableBlockGas {
                transaction_gas_limit: tx.tx().gas_limit(),
                block_available_gas,
            }
            .into())
        }

        let hash = tx.tx().trie_hash();

        let block = self.evm.block();
        // verify the transaction type is accepted by the current fork.
        if tx.tx().is_eip2930() && !chain_spec.is_curie_active_at_block(block.number) {
            return Err(BlockValidationError::InvalidTx {
                hash,
                error: Box::new(InvalidTransaction::Eip2930NotSupported),
            }
            .into())
        }
        if tx.tx().is_eip1559() && !chain_spec.is_curie_active_at_block(block.number) {
            return Err(BlockValidationError::InvalidTx {
                hash,
                error: Box::new(InvalidTransaction::Eip1559NotSupported),
            }
            .into())
        }
        if tx.tx().is_eip4844() {
            return Err(BlockValidationError::InvalidTx {
                hash,
                error: Box::new(InvalidTransaction::Eip4844NotSupported),
            }
            .into())
        }
        if tx.tx().is_eip7702() && !chain_spec.is_euclid_v2_active_at_timestamp(block.timestamp) {
            return Err(BlockValidationError::InvalidTx {
                hash,
                error: Box::new(InvalidTransaction::Eip7702NotSupported),
            }
            .into())
        }

        // disable the base fee and nonce checks for l1 messages.
        self.evm.with_base_fee_check(!is_l1_message);
        self.evm.with_nonce_check(!is_l1_message);

        // execute the transaction and commit the result to the database
        let ResultAndState { result, state } =
            self.evm.transact(tx).map_err(move |err| BlockExecutionError::evm(err, hash))?;

        if !f(&result).should_commit() {
            return Ok(None)
        };

        let l1_fee = if is_l1_message {
            U256::ZERO
        } else {
            // compute l1 fee for all non-l1 transaction
            self.evm.l1_fee().expect("l1 fee loaded")
        };

        let gas_used = result.gas_used();
        self.gas_used += gas_used;

        let ctx = ReceiptBuilderCtx::<'_, Self::Transaction, E> {
            tx: tx.tx(),
            result,
            cumulative_gas_used: self.gas_used,
            l1_fee,
        };
        self.receipts.push(self.receipt_builder.build_receipt(ctx));

        self.evm.db_mut().commit(state);

        Ok(Some(gas_used))
    }

    fn finish(self) -> Result<(Self::Evm, BlockExecutionResult<R::Receipt>), BlockExecutionError> {
        Ok((
            self.evm,
            BlockExecutionResult {
                receipts: self.receipts,
                requests: Default::default(),
                gas_used: self.gas_used,
            },
        ))
    }

    fn set_state_hook(&mut self, _hook: Option<Box<dyn OnStateHook>>) {}

    fn evm_mut(&mut self) -> &mut Self::Evm {
        &mut self.evm
    }

    fn evm(&self) -> &Self::Evm {
        &self.evm
    }
}

/// An extension of the [`Evm`] trait for Scroll.
pub trait EvmExt: Evm {
    /// Sets whether the evm should enable or disable the base fee checks.
    fn with_base_fee_check(&mut self, enabled: bool);
    /// Sets whether the evm should enable or disable the nonce checks.
    fn with_nonce_check(&mut self, enabled: bool);
    /// Returns the l1 fee for the transaction.
    fn l1_fee(&self) -> Option<U256>;
}

impl<DB, I, P> EvmExt for ScrollEvm<DB, I, P>
where
    DB: Database,
    I: Inspector<ScrollContext<DB>>,
    P: PrecompileProvider<ScrollContext<DB>, Output = InterpreterResult>,
{
    fn with_base_fee_check(&mut self, enabled: bool) {
        self.ctx_mut().cfg.disable_base_fee = !enabled;
    }

    fn with_nonce_check(&mut self, enabled: bool) {
        self.ctx_mut().cfg.disable_nonce_check = !enabled;
    }

    fn l1_fee(&self) -> Option<U256> {
        let l1_block_info = &self.ctx().chain;
        let transaction_rlp_bytes = self.ctx().tx.rlp_bytes.as_ref()?;
        let compression_ratio = self.ctx().tx.compression_ratio;
        Some(l1_block_info.calculate_tx_l1_cost(
            transaction_rlp_bytes,
            self.ctx().cfg.spec,
            compression_ratio,
        ))
    }
}

/// Scroll block executor factory.
#[derive(Debug, Clone, Default, Copy)]
pub struct ScrollBlockExecutorFactory<R, Spec = ScrollHardfork, EvmFactory = ScrollEvmFactory> {
    /// Receipt builder.
    receipt_builder: R,
    /// Chain specification.
    spec: Spec,
    /// EVM factory.
    evm_factory: EvmFactory,
}

impl<R, Spec, EvmFactory> ScrollBlockExecutorFactory<R, Spec, EvmFactory> {
    /// Creates a new [`ScrollBlockExecutorFactory`] with the given receipt builder, spec and
    /// factory.
    pub const fn new(receipt_builder: R, spec: Spec, evm_factory: EvmFactory) -> Self {
        Self { receipt_builder, spec, evm_factory }
    }

    /// Exposes the receipt builder.
    pub const fn receipt_builder(&self) -> &R {
        &self.receipt_builder
    }

    /// Exposes the chain specification.
    pub const fn spec(&self) -> &Spec {
        &self.spec
    }

    /// Exposes the EVM factory.
    pub const fn evm_factory(&self) -> &EvmFactory {
        &self.evm_factory
    }
}

impl<R, Spec> BlockExecutorFactory for ScrollBlockExecutorFactory<R, Spec>
where
    R: ScrollReceiptBuilder<Transaction: Transaction + Encodable2718, Receipt: TxReceipt>,
    Spec: ScrollHardforks,
    ScrollTransactionIntoTxEnv<TxEnv>:
        FromRecoveredTx<R::Transaction> + FromTxWithEncoded<R::Transaction>,
    Self: 'static,
{
    type EvmFactory = ScrollEvmFactory;
    type ExecutionCtx<'a> = ScrollBlockExecutionCtx;
    type Transaction = R::Transaction;
    type Receipt = R::Receipt;

    fn evm_factory(&self) -> &Self::EvmFactory {
        &self.evm_factory
    }

    fn create_executor<'a, DB, I>(
        &'a self,
        evm: <ScrollEvmFactory as EvmFactory>::Evm<&'a mut State<DB>, I>,
        ctx: Self::ExecutionCtx<'a>,
    ) -> impl BlockExecutorFor<'a, Self, DB, I>
    where
        DB: Database + 'a,
        I: Inspector<ScrollContext<&'a mut State<DB>>> + 'a,
    {
        ScrollBlockExecutor::new(evm, ctx, &self.spec, &self.receipt_builder)
    }
}
