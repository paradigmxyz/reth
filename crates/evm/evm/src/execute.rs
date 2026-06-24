//! Traits for execution.

use crate::{ConfigureEvm, EvmEnv, TxEnvFor};
use alloc::{sync::Arc, vec::Vec};
use alloy_consensus::{
    transaction::{Either, Recovered},
    Header,
};
use alloy_eips::eip2718::WithEncoded;
use alloy_primitives::{Address, Bytes, B256};
use core::fmt::Debug;
pub use reth_execution_errors::{
    BlockExecutionError, BlockValidationError, InternalBlockExecutionError,
};
pub use reth_execution_types::{BlockExecutionOutput, ExecutionOutcome};
use reth_execution_types::{BlockExecutionResult, HashedPostState};
use reth_primitives_traits::{
    Block, HeaderTy, NodePrimitives, ReceiptTy, RecoveredBlock, SealedHeader, TxTy,
};
use reth_storage_api::StateProvider;

/// Converts a value into the transaction environment expected by an EVM configuration.
pub trait IntoTxEnv<TxEnv> {
    /// Converts this value into the configured transaction environment.
    fn into_tx_env(self) -> TxEnv;
}

impl<TxEnv> IntoTxEnv<TxEnv> for TxEnv {
    fn into_tx_env(self) -> TxEnv {
        self
    }
}

/// Controls how execution produces trie-ready hashed post-state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashedStateMode {
    /// Accumulate final hashed post-state in the returned block execution output.
    OutputOnly,
    /// Stream hashed state updates to the provided hook without accumulating output hashed state.
    StreamOnly,
    /// Accumulate final output hashed state and stream each committed update.
    OutputAndStream,
}

impl HashedStateMode {
    /// Returns true if execution should include hashed state in its output.
    pub const fn output(self) -> bool {
        matches!(self, Self::OutputOnly | Self::OutputAndStream)
    }

    /// Returns true if execution should stream hashed state updates.
    pub const fn stream(self) -> bool {
        matches!(self, Self::StreamOnly | Self::OutputAndStream)
    }
}

/// A configured block executor.
pub trait BlockExecutor: Sized {
    /// The primitive types used by the executor.
    type Primitives: NodePrimitives;
    /// Transaction environment consumed by this executor.
    type Transaction;
    /// The error type returned by the executor.
    type Error: core::error::Error + Send + Sync + 'static;

    /// Applies pre-execution block changes.
    fn apply_pre_execution_changes<H>(
        &mut self,
        on_hashed_state_update: &mut H,
    ) -> Result<(), Self::Error>
    where
        H: FnMut(HashedPostState);

    /// Executes a transaction.
    fn execute_transaction<H>(
        &mut self,
        transaction: Self::Transaction,
        on_hashed_state_update: &mut H,
    ) -> Result<(), Self::Error>
    where
        H: FnMut(HashedPostState);

    /// Returns receipts accumulated so far.
    fn receipts(&self) -> &[ReceiptTy<Self::Primitives>];

    /// Finishes block execution and returns the output.
    fn finish<H>(
        self,
        on_hashed_state_update: &mut H,
    ) -> Result<BlockExecutionOutput<ReceiptTy<Self::Primitives>>, Self::Error>
    where
        H: FnMut(HashedPostState);
}

/// A type that creates configured block executors.
pub trait BlockExecutorFactory: Clone + Debug + Send + Sync + Unpin {
    /// The primitive types used by the factory.
    type Primitives: NodePrimitives;
    /// Transaction environment consumed by executors from this factory.
    type Transaction: Clone + Send + Sync + 'static;
    /// EVM environment consumed by this factory.
    type EvmEnv: EvmEnv;
    /// Execution context for a block or payload.
    type ExecutionCtx<'a>: Debug + Clone + Send
    where
        Self: 'a;
    /// Block executor returned by this factory.
    type Executor<'a, DB>: BlockExecutor<
        Primitives = Self::Primitives,
        Transaction = Self::Transaction,
    >
    where
        Self: 'a,
        DB: evm2::evm::Database + Clone + 'static,
        DB::Error: core::error::Error + Send + Sync + 'static;

    /// Creates a configured block executor.
    fn create_executor<'a, DB>(
        &'a self,
        evm: evm2::Evm<evm2::BaseEvmTypes>,
        ctx: Self::ExecutionCtx<'a>,
        hashed_state_mode: HashedStateMode,
    ) -> Self::Executor<'a, DB>
    where
        Self: 'a,
        DB: evm2::evm::Database + Clone + 'static,
        DB::Error: core::error::Error + Send + Sync + 'static;

    /// Returns the transaction shape consumed by the configured EVM.
    fn evm_tx<'a>(
        &self,
        tx: &'a Self::Transaction,
    ) -> &'a <evm2::BaseEvmTypes as evm2::EvmTypes>::Tx;
}

/// Input for block assembly.
#[expect(missing_debug_implementations)]
#[non_exhaustive]
pub struct BlockAssemblerInput<'a, 'b, F: BlockExecutorFactory + 'a, H = Header> {
    /// EVM environment used for block execution.
    pub evm_env: F::EvmEnv,
    /// Execution context used for block execution.
    pub execution_ctx: F::ExecutionCtx<'a>,
    /// Parent block header.
    pub parent: &'a SealedHeader<H>,
    /// Transactions included in the block.
    pub transactions: Vec<TxTy<F::Primitives>>,
    /// Output of block execution.
    pub output: &'b BlockExecutionResult<ReceiptTy<F::Primitives>>,
    /// Provider with access to state.
    pub state_provider: &'b dyn StateProvider,
    /// State root for the assembled block.
    pub state_root: B256,
    /// Block access list hash.
    pub block_access_list_hash: Option<B256>,
}

impl<'a, 'b, F: BlockExecutorFactory + 'a, H> BlockAssemblerInput<'a, 'b, F, H> {
    /// Creates a new [`BlockAssemblerInput`].
    #[expect(clippy::too_many_arguments)]
    pub const fn new(
        evm_env: F::EvmEnv,
        execution_ctx: F::ExecutionCtx<'a>,
        parent: &'a SealedHeader<H>,
        transactions: Vec<TxTy<F::Primitives>>,
        output: &'b BlockExecutionResult<ReceiptTy<F::Primitives>>,
        state_provider: &'b dyn StateProvider,
        state_root: B256,
        block_access_list_hash: Option<B256>,
    ) -> Self {
        Self {
            evm_env,
            execution_ctx,
            parent,
            transactions,
            output,
            state_provider,
            state_root,
            block_access_list_hash,
        }
    }
}

/// A type that assembles a block from execution output.
pub trait BlockAssembler<F: BlockExecutorFactory>: Clone + Debug + Send + Sync + Unpin {
    /// Block produced by the assembler.
    type Block: Block;

    /// Assembles a block from execution output.
    fn assemble_block(
        &self,
        input: BlockAssemblerInput<'_, '_, F, HeaderTy<F::Primitives>>,
    ) -> Result<Self::Block, BlockExecutionError>;
}

/// A type that knows how to execute blocks.
pub trait Executor: Sized {
    /// The primitive types used by the executor.
    type Primitives: NodePrimitives;
    /// The error type returned by the executor.
    type Error: core::error::Error + Send + Sync + 'static;

    /// Executes a single block and returns [`BlockExecutionResult`], without the state changes.
    fn execute_one(
        &mut self,
        block: &RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>,
    ) -> Result<BlockExecutionResult<<Self::Primitives as NodePrimitives>::Receipt>, Self::Error>;

    /// Consumes the type and executes the block.
    fn execute(
        self,
        block: &RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>,
    ) -> Result<BlockExecutionOutput<<Self::Primitives as NodePrimitives>::Receipt>, Self::Error>;

    /// Executes multiple inputs in the batch and returns an aggregated [`ExecutionOutcome`].
    fn execute_batch<'a, I>(
        self,
        blocks: I,
    ) -> Result<ExecutionOutcome<<Self::Primitives as NodePrimitives>::Receipt>, Self::Error>
    where
        I: IntoIterator<Item = &'a RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>>;

    /// The size hint of the batch's tracked state size.
    fn size_hint(&self) -> usize;

    /// Takes the encoded block access list from executor.
    fn take_bal(&mut self) -> Option<Bytes>;
}

/// Executor returned for the parked legacy executor APIs.
#[expect(missing_debug_implementations)]
pub struct UnsupportedExecutor<N> {
    _marker: core::marker::PhantomData<N>,
}

impl<N> Default for UnsupportedExecutor<N> {
    fn default() -> Self {
        Self { _marker: core::marker::PhantomData }
    }
}

impl<N: NodePrimitives> Executor for UnsupportedExecutor<N> {
    type Primitives = N;
    type Error = BlockExecutionError;

    fn execute_one(
        &mut self,
        _block: &RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>,
    ) -> Result<BlockExecutionResult<<Self::Primitives as NodePrimitives>::Receipt>, Self::Error>
    {
        Err(BlockExecutionError::msg("legacy executor block execution is parked"))
    }

    fn execute(
        self,
        _block: &RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>,
    ) -> Result<BlockExecutionOutput<<Self::Primitives as NodePrimitives>::Receipt>, Self::Error>
    {
        Err(BlockExecutionError::msg("legacy executor block execution is parked"))
    }

    fn execute_batch<'a, I>(
        self,
        _blocks: I,
    ) -> Result<ExecutionOutcome<<Self::Primitives as NodePrimitives>::Receipt>, Self::Error>
    where
        I: IntoIterator<Item = &'a RecoveredBlock<<Self::Primitives as NodePrimitives>::Block>>,
    {
        Err(BlockExecutionError::msg("legacy executor block execution is parked"))
    }

    fn size_hint(&self) -> usize {
        0
    }

    fn take_bal(&mut self) -> Option<Bytes> {
        None
    }
}

/// Helper trait to abstract over recovered transaction wrappers.
#[auto_impl::auto_impl(&)]
pub trait RecoveredTx<T> {
    /// Returns the transaction.
    fn tx(&self) -> &T;

    /// Returns the signer of the transaction.
    fn signer(&self) -> &Address;
}

impl<T> RecoveredTx<T> for Recovered<&T> {
    fn tx(&self) -> &T {
        self.inner()
    }

    fn signer(&self) -> &Address {
        self.signer_ref()
    }
}

impl<T> RecoveredTx<T> for Recovered<Arc<T>> {
    fn tx(&self) -> &T {
        self.inner().as_ref()
    }

    fn signer(&self) -> &Address {
        self.signer_ref()
    }
}

impl<T> RecoveredTx<T> for Recovered<T> {
    fn tx(&self) -> &T {
        self.inner()
    }

    fn signer(&self) -> &Address {
        self.signer_ref()
    }
}

impl<Tx, T: RecoveredTx<Tx>> RecoveredTx<Tx> for WithEncoded<T> {
    fn tx(&self) -> &Tx {
        self.1.tx()
    }

    fn signer(&self) -> &Address {
        self.1.signer()
    }
}

impl<L, R, Tx> RecoveredTx<Tx> for Either<L, R>
where
    L: RecoveredTx<Tx>,
    R: RecoveredTx<Tx>,
{
    fn tx(&self) -> &Tx {
        match self {
            Self::Left(l) => l.tx(),
            Self::Right(r) => r.tx(),
        }
    }

    fn signer(&self) -> &Address {
        match self {
            Self::Left(l) => l.signer(),
            Self::Right(r) => r.signer(),
        }
    }
}

impl<Tx, T: RecoveredTx<Tx>> RecoveredTx<Tx> for Arc<T> {
    fn tx(&self) -> &Tx {
        (**self).tx()
    }

    fn signer(&self) -> &Address {
        (**self).signer()
    }
}

/// Helper trait to split an executable transaction into an EVM transaction environment and its
/// recovered transaction accessor.
pub trait ExecutableTxParts<TxEnv, T> {
    /// The recovered transaction accessor type.
    type Recovered: RecoveredTx<T>;

    /// Converts the transaction into the executable transaction environment and recovered
    /// transaction accessor.
    fn into_parts(self) -> (TxEnv, Self::Recovered);
}

impl<T: Clone, TxEnv> ExecutableTxParts<TxEnv, T> for Recovered<T>
where
    TxEnv: From<Self>,
{
    type Recovered = Self;

    fn into_parts(self) -> (TxEnv, Self) {
        (self.clone().into(), self)
    }
}

impl<T: Clone, TxEnv> ExecutableTxParts<TxEnv, T> for WithEncoded<Recovered<T>>
where
    TxEnv: From<Recovered<T>>,
{
    type Recovered = Self;

    fn into_parts(self) -> (TxEnv, Self) {
        (self.1.clone().into(), self)
    }
}

impl<L, R, TxEnv, T> ExecutableTxParts<TxEnv, T> for Either<L, R>
where
    L: ExecutableTxParts<TxEnv, T>,
    R: ExecutableTxParts<TxEnv, T>,
{
    type Recovered = Either<L::Recovered, R::Recovered>;

    fn into_parts(self) -> (TxEnv, Self::Recovered) {
        match self {
            Self::Left(l) => {
                let (tx_env, recovered) = l.into_parts();
                (tx_env, Either::Left(recovered))
            }
            Self::Right(r) => {
                let (tx_env, recovered) = r.into_parts();
                (tx_env, Either::Right(recovered))
            }
        }
    }
}

/// A helper trait marking a 'static type that can be converted into an [`ExecutableTxParts`] for
/// block executor.
pub trait ExecutableTxFor<Evm: ConfigureEvm>:
    ExecutableTxParts<TxEnvFor<Evm>, TxTy<Evm::Primitives>> + RecoveredTx<TxTy<Evm::Primitives>>
{
}

impl<T, Evm: ConfigureEvm> ExecutableTxFor<Evm> for T where
    T: ExecutableTxParts<TxEnvFor<Evm>, TxTy<Evm::Primitives>> + RecoveredTx<TxTy<Evm::Primitives>>
{
}

/// A transaction stored together with its `TxEnv`.
///
/// See also [`ExecutableTxParts`] for types that can be split into a transaction environment and
/// recovered transaction.
#[derive(Debug)]
pub struct WithTxEnv<TxEnv, T> {
    /// The transaction environment for EVM.
    pub tx_env: TxEnv,
    /// The recovered transaction.
    pub tx: Arc<T>,
}

impl<TxEnv, T> WithTxEnv<TxEnv, T> {
    /// Creates a transaction/environment pair from a type that can be split with
    /// [`ExecutableTxParts::into_parts`].
    pub fn new<Tx, InnerTx>(tx: Tx) -> Self
    where
        Tx: ExecutableTxParts<TxEnv, InnerTx, Recovered = T>,
    {
        let (tx_env, tx) = tx.into_parts();
        Self { tx_env, tx: Arc::new(tx) }
    }
}

impl<TxEnv: Clone, T> Clone for WithTxEnv<TxEnv, T> {
    fn clone(&self) -> Self {
        Self { tx_env: self.tx_env.clone(), tx: self.tx.clone() }
    }
}

impl<TxEnv, Tx, T: RecoveredTx<Tx>> RecoveredTx<Tx> for WithTxEnv<TxEnv, T> {
    fn tx(&self) -> &Tx {
        self.tx.tx()
    }

    fn signer(&self) -> &Address {
        self.tx.signer()
    }
}

impl<TxEnv, T: RecoveredTx<Tx>, Tx> ExecutableTxParts<TxEnv, Tx> for WithTxEnv<TxEnv, T> {
    type Recovered = Arc<T>;

    fn into_parts(self) -> (TxEnv, Self::Recovered) {
        (self.tx_env, self.tx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_ethereum_primitives::EthPrimitives;

    #[test]
    fn unsupported_executor_returns_error() {
        let executor = UnsupportedExecutor::<EthPrimitives>::default();
        let err = executor.execute(&Default::default()).unwrap_err();
        assert!(err.to_string().contains("parked"));
    }
}
