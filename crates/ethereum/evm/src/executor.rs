//! EVM-backed Ethereum executor.

use crate::{
    block_env_with_blob_params,
    execution::{BlockExecutionInput, ExecutionHooks},
    spec_id, BlockExecutionContext, BlockSystemCalls, EthExecutionError, HashedStateMode,
    PayloadExecutionError,
};
use alloc::{rc::Rc, sync::Arc, vec::Vec};
use alloy_consensus::{transaction::Recovered, Header};
use alloy_primitives::Bytes;
use core::cell::RefCell;
use evm2::{
    ethereum::RecoveredTxEnvelope,
    evm::{CacheDB, Database, Db, DbErrorCode, DbResult, DynDatabase, StateChangeSource},
};
use reth_chainspec::{EthChainSpec, EthExecutorSpec};
use reth_ethereum_forks::Hardforks;
use reth_ethereum_primitives::{Block, EthPrimitives, Receipt, TransactionSigned};
use reth_evm::{
    execute::{BlockExecutionOutput, ExecutionOutcome, Executor},
    precompile_cache::{CachedPrecompileProvider, PrecompileCacheMap},
    EvmEnv,
};
use reth_execution_types::BlockExecutionResult;
use reth_primitives_traits::{BlockBody, RecoveredBlock};
use reth_trie_common::HashedPostState;

/// EVM-backed Ethereum block executor.
#[derive(Debug, Clone)]
pub struct EthExecutor<C, DB> {
    chain_spec: Arc<C>,
    database: DB,
    precompile_cache_map: PrecompileCacheMap<evm2::SpecId>,
}

impl<C, DB> EthExecutor<C, DB> {
    /// Creates a new EVM-backed Ethereum executor.
    pub const fn new(
        chain_spec: Arc<C>,
        database: DB,
        precompile_cache_map: PrecompileCacheMap<evm2::SpecId>,
    ) -> Self {
        Self { chain_spec, database, precompile_cache_map }
    }
}

impl<C, DB> EthExecutor<C, DB>
where
    C: EthExecutorSpec + EthChainSpec<Header = Header> + Hardforks + 'static,
    DB: Database + Clone + 'static,
    DB::Error: core::error::Error + Send + Sync + 'static,
{
    fn execute_block(
        &self,
        block: &RecoveredBlock<Block>,
    ) -> Result<BlockExecutionOutput<Receipt>, EthExecutionError<DB::Error>> {
        self.execute_block_with_database::<DB>(block, Db::new(self.database.clone()))
    }

    fn execute_block_with_database<ErrorDB>(
        &self,
        block: &RecoveredBlock<Block>,
        database: impl DynDatabase + 'static,
    ) -> Result<BlockExecutionOutput<Receipt>, EthExecutionError<ErrorDB::Error>>
    where
        ErrorDB: Database + 'static,
    {
        let header = block.header();
        let transactions = block
            .senders_iter()
            .zip(block.body().transactions())
            .map(|(signer, tx)| Recovered::new_unchecked(tx.clone(), *signer))
            .collect::<Vec<TransactionSignedWithSigner>>();
        let context = BlockExecutionContext {
            chain_id: self.chain_spec.chain_id(),
            system_calls: Some(BlockSystemCalls {
                parent_hash: header.parent_hash,
                parent_beacon_block_root: header.parent_beacon_block_root,
            }),
            ommers: Some(&block.body().ommers),
            withdrawals: block.body().withdrawals().map(|withdrawals| withdrawals.as_slice()),
            deposit_contract_address: self.chain_spec.deposit_contract_address(),
        };
        let spec_id = spec_id(self.chain_spec.as_ref(), header);

        BlockExecutionInput::new(
            spec_id,
            block_env_with_blob_params(
                header,
                self.chain_spec.as_ref().blob_params_at_timestamp(header.timestamp),
            ),
            database,
            header.number,
            context,
            alloc::boxed::Box::new(CachedPrecompileProvider::new(
                evm2::Precompiles::base(spec_id),
                self.precompile_cache_map.clone(),
                spec_id,
                None,
            )),
        )
        .execute_recovered_transactions::<ErrorDB>(transactions)
    }
}

impl<C, DB> Executor for EthExecutor<C, DB>
where
    C: EthExecutorSpec + EthChainSpec<Header = Header> + Hardforks + 'static,
    DB: Database + Clone + 'static,
    DB::Error: core::error::Error + Send + Sync + 'static,
{
    type Primitives = EthPrimitives;
    type Error = EthExecutionError<DB::Error>;

    fn execute_one(
        &mut self,
        block: &RecoveredBlock<Block>,
    ) -> Result<BlockExecutionResult<Receipt>, Self::Error> {
        self.execute_block(block).map(|output| output.result)
    }

    fn execute(
        self,
        block: &RecoveredBlock<Block>,
    ) -> Result<BlockExecutionOutput<Receipt>, Self::Error> {
        self.execute_block(block)
    }

    fn execute_batch<'a, I>(self, blocks: I) -> Result<ExecutionOutcome<Receipt>, Self::Error>
    where
        I: IntoIterator<Item = &'a RecoveredBlock<Block>>,
    {
        let mut blocks = blocks.into_iter();
        let Some(block) = blocks.next() else { return Ok(ExecutionOutcome::default()) };

        let first_block = block.header().number;
        let database = SharedBatchDatabase::new(self.database.clone());
        let mut states = Vec::new();
        let mut results = Vec::new();

        let output = self.execute_block_with_database::<DB>(block, database.clone())?;
        database.commit_source(&output.state);
        results.push(output.result);
        states.push(output.state.into_inner());

        for block in blocks {
            let output = self.execute_block_with_database::<DB>(block, database.clone())?;
            database.commit_source(&output.state);
            results.push(output.result);
            states.push(output.state.into_inner());
        }

        Ok(ExecutionOutcome::from_block_states(first_block, states, results))
    }

    fn size_hint(&self) -> usize {
        0
    }

    fn take_bal(&mut self) -> Option<Bytes> {
        None
    }
}

/// Specialized payload executor hooks used by engine payload validation.
pub trait EthPayloadExecutor {
    /// Error returned by payload execution.
    type Error: core::error::Error + Send + Sync + 'static;

    /// Returns this executor configured with the provided precompile cache.
    fn with_precompile_cache_map(
        self,
        precompile_cache_map: PrecompileCacheMap<evm2::SpecId>,
    ) -> Self
    where
        Self: Sized;

    /// Executes EVM-native transaction envelopes with progress and hashed-state hooks.
    fn execute_payload_with_hashed_state_hook<I, F, H, Env>(
        self,
        evm_env: Env,
        transactions: I,
        context: BlockExecutionContext<'_>,
        on_transaction_executed: F,
        on_hashed_state_update: H,
        hashed_state_mode: HashedStateMode,
    ) -> Result<BlockExecutionOutput<Receipt>, Self::Error>
    where
        Env: EvmEnv,
        I: IntoIterator<Item = RecoveredTxEnvelope>,
        F: FnMut(usize),
        H: FnMut(HashedPostState);

    /// Executes a fallible stream of EVM-native transaction envelopes with progress and
    /// hashed-state hooks.
    fn execute_payload_with_fallible_hashed_state_hook<I, F, H, Env, TxErr>(
        self,
        evm_env: Env,
        transactions: I,
        context: BlockExecutionContext<'_>,
        on_transaction_executed: F,
        on_hashed_state_update: H,
        hashed_state_mode: HashedStateMode,
    ) -> Result<BlockExecutionOutput<Receipt>, PayloadExecutionError<Self::Error, TxErr>>
    where
        Env: EvmEnv,
        I: IntoIterator<Item = Result<RecoveredTxEnvelope, TxErr>>,
        TxErr: core::error::Error + Send + Sync + 'static,
        F: FnMut(usize),
        H: FnMut(HashedPostState);

    /// Executes a fallible stream of EVM-native transaction envelopes with progress, receipt, and
    /// hashed-state hooks.
    #[expect(clippy::too_many_arguments)]
    fn execute_payload_with_fallible_receipt_hashed_state_hook<I, F, R, H, Env, TxErr, ReceiptErr>(
        self,
        evm_env: Env,
        transactions: I,
        context: BlockExecutionContext<'_>,
        on_transaction_executed: F,
        on_receipt: R,
        on_hashed_state_update: H,
        hashed_state_mode: HashedStateMode,
    ) -> Result<BlockExecutionOutput<Receipt>, PayloadExecutionError<Self::Error, TxErr, ReceiptErr>>
    where
        Env: EvmEnv,
        I: IntoIterator<Item = Result<RecoveredTxEnvelope, TxErr>>,
        TxErr: core::error::Error + Send + Sync + 'static,
        ReceiptErr: core::error::Error + Send + Sync + 'static,
        F: FnMut(usize),
        R: FnMut(usize, &Receipt) -> Result<(), ReceiptErr>,
        H: FnMut(HashedPostState);
}

impl<C, DB> EthPayloadExecutor for EthExecutor<C, DB>
where
    C: EthExecutorSpec + EthChainSpec<Header = Header> + Hardforks + 'static,
    DB: Database + Clone + 'static,
    DB::Error: core::error::Error + Send + Sync + 'static,
{
    type Error = EthExecutionError<DB::Error>;

    fn with_precompile_cache_map(
        mut self,
        precompile_cache_map: PrecompileCacheMap<evm2::SpecId>,
    ) -> Self {
        self.precompile_cache_map = precompile_cache_map;
        self
    }

    fn execute_payload_with_hashed_state_hook<I, F, H, Env>(
        self,
        evm_env: Env,
        transactions: I,
        context: BlockExecutionContext<'_>,
        on_transaction_executed: F,
        on_hashed_state_update: H,
        hashed_state_mode: HashedStateMode,
    ) -> Result<BlockExecutionOutput<Receipt>, Self::Error>
    where
        Env: EvmEnv,
        I: IntoIterator<Item = RecoveredTxEnvelope>,
        F: FnMut(usize),
        H: FnMut(HashedPostState),
    {
        self.execute_payload_with_fallible_hashed_state_hook(
            evm_env,
            transactions.into_iter().map(Ok::<_, core::convert::Infallible>),
            context,
            on_transaction_executed,
            on_hashed_state_update,
            hashed_state_mode,
        )
        .map_err(|err| match err {
            PayloadExecutionError::Execution(err) => err,
            PayloadExecutionError::Transaction(err) | PayloadExecutionError::Receipt(err) => {
                match err {}
            }
        })
    }

    fn execute_payload_with_fallible_hashed_state_hook<I, F, H, Env, TxErr>(
        self,
        evm_env: Env,
        transactions: I,
        context: BlockExecutionContext<'_>,
        on_transaction_executed: F,
        on_hashed_state_update: H,
        hashed_state_mode: HashedStateMode,
    ) -> Result<BlockExecutionOutput<Receipt>, PayloadExecutionError<Self::Error, TxErr>>
    where
        Env: EvmEnv,
        I: IntoIterator<Item = Result<RecoveredTxEnvelope, TxErr>>,
        TxErr: core::error::Error + Send + Sync + 'static,
        F: FnMut(usize),
        H: FnMut(HashedPostState),
    {
        self.execute_payload_with_fallible_receipt_hashed_state_hook(
            evm_env,
            transactions,
            context,
            on_transaction_executed,
            |_, _| Ok::<(), core::convert::Infallible>(()),
            on_hashed_state_update,
            hashed_state_mode,
        )
        .map_err(|err| match err {
            PayloadExecutionError::Execution(err) => PayloadExecutionError::Execution(err),
            PayloadExecutionError::Transaction(err) => PayloadExecutionError::Transaction(err),
            PayloadExecutionError::Receipt(err) => match err {},
        })
    }

    fn execute_payload_with_fallible_receipt_hashed_state_hook<I, F, R, H, Env, TxErr, ReceiptErr>(
        self,
        evm_env: Env,
        transactions: I,
        context: BlockExecutionContext<'_>,
        on_transaction_executed: F,
        on_receipt: R,
        on_hashed_state_update: H,
        hashed_state_mode: HashedStateMode,
    ) -> Result<BlockExecutionOutput<Receipt>, PayloadExecutionError<Self::Error, TxErr, ReceiptErr>>
    where
        Env: EvmEnv,
        I: IntoIterator<Item = Result<RecoveredTxEnvelope, TxErr>>,
        TxErr: core::error::Error + Send + Sync + 'static,
        ReceiptErr: core::error::Error + Send + Sync + 'static,
        F: FnMut(usize),
        R: FnMut(usize, &Receipt) -> Result<(), ReceiptErr>,
        H: FnMut(HashedPostState),
    {
        let spec_id = evm_env.spec_id();
        let block_env = evm_env.block_env();
        let block_number = block_env.number.to::<u64>();

        BlockExecutionInput::new(
            spec_id,
            block_env,
            Db::new(self.database),
            block_number,
            context,
            alloc::boxed::Box::new(CachedPrecompileProvider::new(
                evm2::Precompiles::base(spec_id),
                self.precompile_cache_map,
                spec_id,
                None,
            )),
        )
        .execute_fallible_envelopes::<DB, TxErr, ReceiptErr, _, _, _, _>(
            transactions,
            ExecutionHooks::new(
                on_transaction_executed,
                on_receipt,
                on_hashed_state_update,
                hashed_state_mode,
            ),
        )
    }
}

type TransactionSignedWithSigner = Recovered<TransactionSigned>;

struct SharedBatchDatabase<DB: Database> {
    inner: Rc<RefCell<CacheDB<Db<DB>>>>,
}

impl<DB: Database> Clone for SharedBatchDatabase<DB> {
    fn clone(&self) -> Self {
        Self { inner: Rc::clone(&self.inner) }
    }
}

impl<DB> SharedBatchDatabase<DB>
where
    DB: Database,
{
    fn new(database: DB) -> Self {
        Self { inner: Rc::new(RefCell::new(CacheDB::new(Db::new(database)))) }
    }

    fn commit_source<S: StateChangeSource>(&self, source: &S) {
        self.inner.borrow_mut().commit_source(source);
    }
}

impl<DB> DynDatabase for SharedBatchDatabase<DB>
where
    DB: Database + 'static,
{
    fn get_account(
        &mut self,
        address: &alloy_primitives::Address,
    ) -> DbResult<Option<evm2::evm::AccountInfo>> {
        self.inner.borrow_mut().get_account(address)
    }

    fn get_code_by_hash(
        &mut self,
        code_hash: &alloy_primitives::B256,
    ) -> DbResult<evm2::bytecode::Bytecode> {
        self.inner.borrow_mut().get_code_by_hash(code_hash)
    }

    fn get_storage(
        &mut self,
        address: &alloy_primitives::Address,
        key: &evm2::interpreter::Word,
    ) -> DbResult<evm2::interpreter::Word> {
        self.inner.borrow_mut().get_storage(address, key)
    }

    fn get_block_hash(
        &mut self,
        number: &evm2::interpreter::Word,
    ) -> DbResult<Option<alloy_primitives::B256>> {
        self.inner.borrow_mut().get_block_hash(number)
    }

    fn error(&mut self, code: DbErrorCode) -> alloc::boxed::Box<dyn core::error::Error> {
        self.inner.borrow_mut().error(code)
    }
}
