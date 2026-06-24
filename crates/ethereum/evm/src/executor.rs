//! EVM-backed Ethereum executor.

use crate::{
    convert::{block_env_with_blob_params, spec_id},
    execution::{
        block_requests_from_receipts, execute_transaction, post_block_balance_state_changes,
        post_execution_system_call_state_changes, pre_execution_system_call_state_changes,
        BlockExecutionInput,
    },
    BlockExecutionContext, BlockSystemCalls, EthBlockExecutionCtx, EthExecutionError, EthTxEnv,
    HashedStateMode, RethReceiptBuilder,
};
use alloc::{borrow::Cow, rc::Rc, sync::Arc, vec::Vec};
use alloy_consensus::{transaction::Recovered, Header, TxType};
use alloy_eips::{eip2718::Typed2718, eip4895::Withdrawal};
use alloy_primitives::{Address, Bytes, B256};
use core::cell::RefCell;
use evm2::{
    evm::{
        BlockStateAccumulator, CacheDB, Database, Db, DbErrorCode, DbResult, DynDatabase,
        StateChangeSource,
    },
    interpreter::Host,
    BaseEvmTypes, Evm,
};
use reth_chainspec::{EthChainSpec, EthExecutorSpec};
use reth_ethereum_forks::Hardforks;
use reth_ethereum_primitives::{Block, EthPrimitives, Receipt, TransactionSigned};
use reth_evm::{
    execute::{BlockExecutionOutput, BlockExecutor, ExecutionOutcome, Executor},
    precompile_cache::{CachedPrecompileProvider, PrecompileCacheMap},
};
use reth_execution_types::{BlockExecutionResult, HashedPostStateSink};
use reth_primitives_traits::{BlockBody, RecoveredBlock};
use reth_trie_common::{HashedPostState, KeccakKeyHasher};

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

/// Configured Ethereum block executor backed by evm2.
#[expect(missing_debug_implementations)]
pub struct EthBlockExecutor<'a, DB> {
    evm: Evm<BaseEvmTypes>,
    spec_id: evm2::SpecId,
    block_number: u64,
    block_beneficiary: Address,
    parent_hash: B256,
    parent_beacon_block_root: Option<B256>,
    ommers: &'a [Header],
    withdrawals: Option<Cow<'a, [Withdrawal]>>,
    chain_id: u64,
    deposit_contract_address: Option<Address>,
    block_state: BlockStateAccumulator,
    hashed_state: Option<HashedPostStateSink<KeccakKeyHasher>>,
    hashed_state_mode: HashedStateMode,
    receipts: Vec<Receipt>,
    cumulative_gas_used: u64,
    _database: core::marker::PhantomData<DB>,
}

impl<'a, DB> EthBlockExecutor<'a, DB>
where
    DB: Database + 'static,
{
    /// Creates a configured Ethereum block executor.
    pub(crate) fn new(
        mut evm: Evm<BaseEvmTypes>,
        context: EthBlockExecutionCtx<'a>,
        chain_id: u64,
        deposit_contract_address: Option<alloy_primitives::Address>,
        hashed_state_mode: HashedStateMode,
    ) -> Self {
        let spec_id = evm.spec_id();
        let block = *evm.block_env();
        let block_number = block.number.to::<u64>();
        let block_beneficiary = block.beneficiary;

        Self {
            evm,
            spec_id,
            block_number,
            block_beneficiary,
            parent_hash: context.parent_hash,
            parent_beacon_block_root: context.parent_beacon_block_root,
            ommers: context.ommers,
            withdrawals: context.withdrawals,
            chain_id,
            deposit_contract_address,
            block_state: BlockStateAccumulator::new(),
            hashed_state: hashed_state_mode
                .output()
                .then(HashedPostStateSink::<KeccakKeyHasher>::default),
            hashed_state_mode,
            receipts: Vec::new(),
            cumulative_gas_used: 0,
            _database: core::marker::PhantomData,
        }
    }

    const fn block_context<'ctx>(
        chain_id: u64,
        deposit_contract_address: Option<Address>,
        parent_hash: B256,
        parent_beacon_block_root: Option<B256>,
        ommers: &'ctx [Header],
        withdrawals: Option<&'ctx [Withdrawal]>,
    ) -> BlockExecutionContext<'ctx> {
        BlockExecutionContext {
            chain_id,
            system_calls: Some(BlockSystemCalls { parent_hash, parent_beacon_block_root }),
            ommers: Some(ommers),
            withdrawals,
            deposit_contract_address,
        }
    }
}

impl<DB> BlockExecutor for EthBlockExecutor<'_, DB>
where
    DB: Database + 'static,
    DB::Error: core::error::Error + Send + Sync + 'static,
{
    type Primitives = EthPrimitives;
    type Transaction = EthTxEnv;
    type Error = EthExecutionError<DB::Error>;

    fn apply_pre_execution_changes<H>(
        &mut self,
        on_hashed_state_update: &mut H,
    ) -> Result<(), Self::Error>
    where
        H: FnMut(HashedPostState),
    {
        let context = Self::block_context(
            self.chain_id,
            self.deposit_contract_address,
            self.parent_hash,
            self.parent_beacon_block_root,
            self.ommers,
            None,
        );
        pre_execution_system_call_state_changes::<DB>(
            &mut self.evm,
            &mut self.block_state,
            self.hashed_state.as_mut(),
            self.hashed_state_mode.stream(),
            on_hashed_state_update,
            self.spec_id,
            self.block_number,
            context,
        )
    }

    fn execute_transaction<H>(
        &mut self,
        transaction: Self::Transaction,
        on_hashed_state_update: &mut H,
    ) -> Result<(), Self::Error>
    where
        H: FnMut(HashedPostState),
    {
        let transaction = transaction.into_envelope();
        let tx_type =
            TxType::try_from(transaction.ty()).expect("transaction envelope has valid type");
        let outcome = execute_transaction::<DB>(
            &mut self.evm,
            &mut self.block_state,
            self.hashed_state.as_mut(),
            self.hashed_state_mode.stream(),
            on_hashed_state_update,
            &transaction,
        )?;
        self.cumulative_gas_used += outcome.gas_used;
        self.receipts.push(RethReceiptBuilder.build_receipt(
            tx_type,
            outcome,
            self.cumulative_gas_used,
        ));
        Ok(())
    }

    fn receipts(&self) -> &[Receipt] {
        &self.receipts
    }

    fn finish<H>(
        mut self,
        on_hashed_state_update: &mut H,
    ) -> Result<BlockExecutionOutput<Receipt>, Self::Error>
    where
        H: FnMut(HashedPostState),
    {
        let context = Self::block_context(
            self.chain_id,
            self.deposit_contract_address,
            self.parent_hash,
            self.parent_beacon_block_root,
            self.ommers,
            None,
        );
        let mut requests =
            block_requests_from_receipts::<DB>(self.spec_id, context, &self.receipts)?;
        let context = Self::block_context(
            self.chain_id,
            self.deposit_contract_address,
            self.parent_hash,
            self.parent_beacon_block_root,
            self.ommers,
            None,
        );
        post_execution_system_call_state_changes::<DB>(
            &mut self.evm,
            &mut self.block_state,
            self.hashed_state.as_mut(),
            self.hashed_state_mode.stream(),
            on_hashed_state_update,
            self.spec_id,
            context,
            &mut requests,
        )?;

        let withdrawals = self.withdrawals.clone();
        let context = Self::block_context(
            self.chain_id,
            self.deposit_contract_address,
            self.parent_hash,
            self.parent_beacon_block_root,
            self.ommers,
            withdrawals.as_deref(),
        );
        post_block_balance_state_changes::<DB>(
            &mut self.evm,
            &mut self.block_state,
            self.hashed_state.as_mut(),
            self.hashed_state_mode.stream(),
            on_hashed_state_update,
            self.spec_id,
            self.block_number,
            self.block_beneficiary,
            context.ommers,
            context.withdrawals,
        )?;

        let mut output = RethReceiptBuilder
            .build_block_output_from_receipts_and_state_with_hashed_state(
                self.receipts,
                self.block_state,
                self.hashed_state.map(HashedPostStateSink::into_hashed_post_state),
            );
        output.result.requests = requests;

        Ok(output)
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
