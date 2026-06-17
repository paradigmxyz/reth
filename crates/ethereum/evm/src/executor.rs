//! evm2-backed Ethereum executor.

use crate::{
    evm2_block_env_with_blob_params,
    evm2_executor::execute_evm2_block_with_dyn_database_context_and_precompiles, evm2_spec,
    Evm2BlockExecutionContext, Evm2BlockSystemCalls, Evm2ExecutionError,
};
use alloc::{rc::Rc, sync::Arc, vec::Vec};
use alloy_consensus::{transaction::Recovered, Header};
use alloy_primitives::Bytes;
use core::cell::RefCell;
use evm2::evm::{CacheDB, Database, Db, DbErrorCode, DbResult, DynDatabase, StateChangeSource};
use reth_chainspec::{EthChainSpec, EthExecutorSpec};
use reth_ethereum_forks::Hardforks;
use reth_ethereum_primitives::{Block, EthPrimitives, Receipt, TransactionSigned};
use reth_evm::{
    evm2_precompile_cache::{CachedPrecompileProvider, PrecompileCacheMap},
    execute::{BlockExecutionOutput, ExecutionOutcome, Executor},
};
use reth_execution_types::BlockExecutionResult;
use reth_primitives_traits::{BlockBody, RecoveredBlock};

/// evm2-backed Ethereum block executor.
#[derive(Debug, Clone)]
pub struct EthEvm2Executor<C, DB> {
    chain_spec: Arc<C>,
    database: DB,
    precompile_cache_map: PrecompileCacheMap<evm2::SpecId>,
}

impl<C, DB> EthEvm2Executor<C, DB> {
    /// Creates a new evm2-backed Ethereum executor.
    pub const fn new(
        chain_spec: Arc<C>,
        database: DB,
        precompile_cache_map: PrecompileCacheMap<evm2::SpecId>,
    ) -> Self {
        Self { chain_spec, database, precompile_cache_map }
    }
}

impl<C, DB> EthEvm2Executor<C, DB>
where
    C: EthExecutorSpec + EthChainSpec<Header = Header> + Hardforks + 'static,
    DB: Database + Clone + 'static,
    DB::Error: core::error::Error + Send + Sync + 'static,
{
    fn execute_block(
        &self,
        block: &RecoveredBlock<Block>,
    ) -> Result<BlockExecutionOutput<Receipt>, Evm2ExecutionError<DB::Error>> {
        self.execute_block_with_database::<DB>(block, Db::new(self.database.clone()))
    }

    fn execute_block_with_database<ErrorDB>(
        &self,
        block: &RecoveredBlock<Block>,
        database: impl DynDatabase + 'static,
    ) -> Result<BlockExecutionOutput<Receipt>, Evm2ExecutionError<ErrorDB::Error>>
    where
        ErrorDB: Database + 'static,
    {
        let header = block.header();
        let transactions = block
            .senders_iter()
            .zip(block.body().transactions())
            .map(|(signer, tx)| Recovered::new_unchecked(tx.clone(), *signer))
            .collect::<Vec<TransactionSignedWithSigner>>();
        let context = Evm2BlockExecutionContext {
            chain_id: self.chain_spec.chain_id(),
            system_calls: Some(Evm2BlockSystemCalls {
                parent_hash: header.parent_hash,
                parent_beacon_block_root: header.parent_beacon_block_root,
            }),
            ommers: Some(&block.body().ommers),
            withdrawals: block.body().withdrawals().map(|withdrawals| withdrawals.as_slice()),
            deposit_contract_address: self.chain_spec.deposit_contract_address(),
        };
        let spec_id = evm2_spec(self.chain_spec.as_ref(), header);

        execute_evm2_block_with_dyn_database_context_and_precompiles::<ErrorDB>(
            spec_id,
            evm2_block_env_with_blob_params(
                header,
                self.chain_spec.as_ref().blob_params_at_timestamp(header.timestamp),
            ),
            database,
            header.number,
            transactions,
            context,
            alloc::boxed::Box::new(CachedPrecompileProvider::new(
                evm2::Precompiles::base(spec_id),
                self.precompile_cache_map.clone(),
                spec_id,
                None,
            )),
        )
    }
}

impl<C, DB> Executor for EthEvm2Executor<C, DB>
where
    C: EthExecutorSpec + EthChainSpec<Header = Header> + Hardforks + 'static,
    DB: Database + Clone + 'static,
    DB::Error: core::error::Error + Send + Sync + 'static,
{
    type Primitives = EthPrimitives;
    type Error = Evm2ExecutionError<DB::Error>;

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
