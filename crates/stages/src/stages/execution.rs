use crate::{
    exec_or_return, ExecAction, ExecInput, ExecOutput, Stage, StageError, StageId, UnwindInput,
    UnwindOutput,
};
use metrics_core::Counter;
use reth_db::{
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO},
    database::Database,
    models::TransitionIdAddress,
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_metrics_derive::Metrics;
use reth_primitives::{Address, Block, BlockNumber, BlockWithSenders, U256};
use reth_provider::{
    post_state::PostState, BlockExecutor, ExecutorFactory, LatestStateProviderRef, Transaction,
};
use std::time::Instant;
use tracing::*;

/// The [`StageId`] of the execution stage.
pub const EXECUTION: StageId = StageId("Execution");

/// Execution stage metrics.
#[derive(Metrics)]
#[metrics(scope = "sync.execution")]
pub struct ExecutionStageMetrics {
    /// The total amount of gas processed (in millions)
    mgas_processed_total: Counter,
}

/// The execution stage executes all transactions and
/// update history indexes.
///
/// Input tables:
/// - [tables::CanonicalHeaders] get next block to execute.
/// - [tables::Headers] get for revm environment variables.
/// - [tables::HeaderTD]
/// - [tables::BlockBodyIndices] to get tx number
/// - [tables::Transactions] to execute
///
/// For state access [LatestStateProviderRef] provides us latest state and history state
/// For latest most recent state [LatestStateProviderRef] would need (Used for execution Stage):
/// - [tables::PlainAccountState]
/// - [tables::Bytecodes]
/// - [tables::PlainStorageState]
///
/// Tables updated after state finishes execution:
/// - [tables::PlainAccountState]
/// - [tables::PlainStorageState]
/// - [tables::Bytecodes]
/// - [tables::AccountChangeSet]
/// - [tables::StorageChangeSet]
///
/// For unwinds we are accessing:
/// - [tables::BlockBodyIndices] get tx index to know what needs to be unwinded
/// - [tables::AccountHistory] to remove change set and apply old values to
/// - [tables::PlainAccountState] [tables::StorageHistory] to remove change set and apply old values
/// to [tables::PlainStorageState]
// false positive, we cannot derive it if !DB: Debug.
#[allow(missing_debug_implementations)]
pub struct ExecutionStage<EF: ExecutorFactory> {
    metrics: ExecutionStageMetrics,
    /// The stage's internal executor
    executor_factory: EF,
    /// Commit threshold
    commit_threshold: u64,
}

impl<EF: ExecutorFactory> ExecutionStage<EF> {
    /// Create new execution stage with specified config.
    pub fn new(executor_factory: EF, commit_threshold: u64) -> Self {
        Self { metrics: ExecutionStageMetrics::default(), executor_factory, commit_threshold }
    }

    /// Create an execution stage with the provided  executor factory.
    ///
    /// The commit threshold will be set to 10_000.
    pub fn new_with_factory(executor_factory: EF) -> Self {
        Self {
            metrics: ExecutionStageMetrics::default(),
            executor_factory,
            commit_threshold: 10_000,
        }
    }

    // TODO: This should be in the block provider trait once we consolidate
    // SharedDatabase/Transaction
    fn read_block_with_senders<DB: Database>(
        tx: &Transaction<'_, DB>,
        block_number: BlockNumber,
    ) -> Result<(BlockWithSenders, U256), StageError> {
        let header = tx.get_header(block_number)?;
        let td = tx.get_td(block_number)?;
        let ommers = tx.get::<tables::BlockOmmers>(block_number)?.unwrap_or_default().ommers;
        let withdrawals = tx.get::<tables::BlockWithdrawals>(block_number)?.map(|v| v.withdrawals);

        let (transactions, senders): (Vec<_>, Vec<_>) = tx
            .get_block_transaction_range(block_number..=block_number)?
            .into_iter()
            .flat_map(|(_, txs)| txs.into_iter())
            .map(|tx| tx.to_components())
            .unzip();

        Ok((Block { header, body: transactions, ommers, withdrawals }.with_senders(senders), td))
    }

    /// Execute the stage.
    pub fn execute_inner<DB: Database>(
        &self,
        tx: &mut Transaction<'_, DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        let ((start_block, end_block), capped) =
            exec_or_return!(input, self.commit_threshold, "sync::stages::execution");
        let last_block = input.stage_progress.unwrap_or_default();

        // Create state provider with cached state
        let mut executor = self.executor_factory.with_sp(LatestStateProviderRef::new(&**tx));

        // Fetch transactions, execute them and generate results
        let mut state = PostState::default();
        for block_number in start_block..=end_block {
            let (block, td) = Self::read_block_with_senders(tx, block_number)?;

            // Configure the executor to use the current state.
            trace!(target: "sync::stages::execution", number = block_number, txs = block.body.len(), "Executing block");
            let (block, senders) = block.into_components();
            let block_state = executor
                .execute_and_verify_receipt(&block, td, Some(senders))
                .map_err(|error| StageError::ExecutionError { block: block_number, error })?;
            if let Some(last_receipt) = block_state.receipts().last() {
                self.metrics
                    .mgas_processed_total
                    .increment(last_receipt.cumulative_gas_used / 1_000_000);
            }
            state.extend(block_state);
        }

        // put execution results to database
        let first_transition_id = tx.get_block_transition(last_block)?;

        let start = Instant::now();
        trace!(target: "sync::stages::execution", changes = state.changes().len(), accounts = state.accounts().len(), "Writing updated state to database");
        state.write_to_db(&**tx, first_transition_id)?;
        trace!(target: "sync::stages::execution", took = ?Instant::now().duration_since(start), "Wrote state");

        let done = !capped;
        info!(target: "sync::stages::execution", stage_progress = end_block, done, "Sync iteration finished");
        Ok(ExecOutput { stage_progress: end_block, done })
    }
}

/// The size of the stack used by the executor.
///
/// Ensure the size is aligned to 8 as this is usually more efficient.
///
/// Currently 64 megabytes.
const BIG_STACK_SIZE: usize = 64 * 1024 * 1024;

#[async_trait::async_trait]
impl<EF: ExecutorFactory, DB: Database> Stage<DB> for ExecutionStage<EF> {
    /// Return the id of the stage
    fn id(&self) -> StageId {
        EXECUTION
    }

    /// Execute the stage
    async fn execute(
        &mut self,
        tx: &mut Transaction<'_, DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        // For Ethereum transactions that reaches the max call depth (1024) revm can use more stack
        // space than what is allocated by default.
        //
        // To make sure we do not panic in this case, spawn a thread with a big stack allocated.
        //
        // A fix in revm is pending to give more insight into the stack size, which we can use later
        // to optimize revm or move data to the heap.
        //
        // See https://github.com/bluealloy/revm/issues/305
        std::thread::scope(|scope| {
            let handle = std::thread::Builder::new()
                .stack_size(BIG_STACK_SIZE)
                .spawn_scoped(scope, || {
                    // execute and store output to results
                    self.execute_inner(tx, input)
                })
                .expect("Expects that thread name is not null");
            handle.join().expect("Expects for thread to not panic")
        })
    }

    /// Unwind the stage.
    async fn unwind(
        &mut self,
        tx: &mut Transaction<'_, DB>,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        info!(target: "sync::stages::execution", to_block = input.unwind_to, "Unwinding");

        // Acquire changeset cursors
        let mut account_changeset = tx.cursor_dup_write::<tables::AccountChangeSet>()?;
        let mut storage_changeset = tx.cursor_dup_write::<tables::StorageChangeSet>()?;

        let from_transition_rev = tx.get_block_transition(input.unwind_to)?;
        let to_transition_rev = tx.get_block_transition(input.stage_progress)?;

        if from_transition_rev > to_transition_rev {
            panic!("Unwind transition {} (stage progress block #{}) is higher than the transition {} of (unwind block #{})", from_transition_rev, input.stage_progress, to_transition_rev, input.unwind_to);
        }
        let num_of_tx = (to_transition_rev - from_transition_rev) as usize;

        // if there is no transaction ids, this means blocks were empty and block reward change set
        // is not present.
        if num_of_tx == 0 {
            return Ok(UnwindOutput { stage_progress: input.unwind_to })
        }

        // get all batches for account change
        // Check if walk and walk_dup would do the same thing
        let account_changeset_batch = account_changeset
            .walk_range(from_transition_rev..to_transition_rev)?
            .collect::<Result<Vec<_>, _>>()?;

        // revert all changes to PlainState
        for (_, changeset) in account_changeset_batch.into_iter().rev() {
            if let Some(account_info) = changeset.info {
                tx.put::<tables::PlainAccountState>(changeset.address, account_info)?;
            } else {
                tx.delete::<tables::PlainAccountState>(changeset.address, None)?;
            }
        }

        // get all batches for storage change
        let storage_changeset_batch = storage_changeset
            .walk_range(
                TransitionIdAddress((from_transition_rev, Address::zero()))..
                    TransitionIdAddress((to_transition_rev, Address::zero())),
            )?
            .collect::<Result<Vec<_>, _>>()?;

        // revert all changes to PlainStorage
        let mut plain_storage_cursor = tx.cursor_dup_write::<tables::PlainStorageState>()?;

        for (key, storage) in storage_changeset_batch.into_iter().rev() {
            let address = key.address();
            if let Some(v) = plain_storage_cursor.seek_by_key_subkey(address, storage.key)? {
                if v.key == storage.key {
                    plain_storage_cursor.delete_current()?;
                }
            }
            if storage.value != U256::ZERO {
                plain_storage_cursor.upsert(address, storage)?;
            }
        }

        // Discard unwinded changesets
        let mut rev_acc_changeset_walker = account_changeset.walk_back(None)?;
        while let Some((transition_id, _)) = rev_acc_changeset_walker.next().transpose()? {
            if transition_id < from_transition_rev {
                break
            }
            // delete all changesets
            tx.delete::<tables::AccountChangeSet>(transition_id, None)?;
        }

        let mut rev_storage_changeset_walker = storage_changeset.walk_back(None)?;
        while let Some((key, _)) = rev_storage_changeset_walker.next().transpose()? {
            if key.transition_id() < from_transition_rev {
                break
            }
            // delete all changesets
            tx.delete::<tables::StorageChangeSet>(key, None)?;
        }

        Ok(UnwindOutput { stage_progress: input.unwind_to })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{TestTransaction, PREV_STAGE_ID};
    use reth_db::{
        mdbx::{test_utils::create_test_db, EnvKind, WriteMap},
        models::AccountBeforeTx,
    };
    use reth_primitives::{
        hex_literal::hex, keccak256, Account, Bytecode, ChainSpecBuilder, SealedBlock,
        StorageEntry, H160, H256, U256,
    };
    use reth_provider::insert_canonical_block;
    use reth_revm::Factory;
    use reth_rlp::Decodable;
    use std::{
        ops::{Deref, DerefMut},
        sync::Arc,
    };

    fn stage() -> ExecutionStage<Factory> {
        let factory =
            Factory::new(Arc::new(ChainSpecBuilder::mainnet().berlin_activated().build()));
        ExecutionStage::new(factory, 100)
    }

    #[tokio::test]
    async fn sanity_execution_of_block() {
        // TODO cleanup the setup after https://github.com/paradigmxyz/reth/issues/332
        // is merged as it has similar framework
        let state_db = create_test_db::<WriteMap>(EnvKind::RW);
        let mut tx = Transaction::new(state_db.as_ref()).unwrap();
        let input = ExecInput {
            previous_stage: Some((PREV_STAGE_ID, 1)),
            /// The progress of this stage the last time it was executed.
            stage_progress: None,
        };
        let mut genesis_rlp = hex!("f901faf901f5a00000000000000000000000000000000000000000000000000000000000000000a01dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347942adc25665018aa1fe0e6bc666dac8fc2697ff9baa045571b40ae66ca7480791bbb2887286e4e4c4b1b298b191c889d6959023a32eda056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421b901000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000083020000808502540be400808000a00000000000000000000000000000000000000000000000000000000000000000880000000000000000c0c0").as_slice();
        let genesis = SealedBlock::decode(&mut genesis_rlp).unwrap();
        let mut block_rlp = hex!("f90262f901f9a075c371ba45999d87f4542326910a11af515897aebce5265d3f6acd1f1161f82fa01dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347942adc25665018aa1fe0e6bc666dac8fc2697ff9baa098f2dcd87c8ae4083e7017a05456c14eea4b1db2032126e27b3b1563d57d7cc0a08151d548273f6683169524b66ca9fe338b9ce42bc3540046c828fd939ae23bcba03f4e5c2ec5b2170b711d97ee755c160457bb58d8daa338e835ec02ae6860bbabb901000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000083020000018502540be40082a8798203e800a00000000000000000000000000000000000000000000000000000000000000000880000000000000000f863f861800a8405f5e10094100000000000000000000000000000000000000080801ba07e09e26678ed4fac08a249ebe8ed680bf9051a5e14ad223e4b2b9d26e0208f37a05f6e3f188e3e6eab7d7d3b6568f5eac7d687b08d307d3154ccd8c87b4630509bc0").as_slice();
        let block = SealedBlock::decode(&mut block_rlp).unwrap();
        insert_canonical_block(tx.deref_mut(), genesis, None, true).unwrap();
        insert_canonical_block(tx.deref_mut(), block.clone(), None, true).unwrap();
        tx.commit().unwrap();

        // insert pre state
        let db_tx = tx.deref_mut();
        let acc1 = H160(hex!("1000000000000000000000000000000000000000"));
        let acc2 = H160(hex!("a94f5374fce5edbc8e2a8697c15331677e6ebf0b"));
        let code = hex!("5a465a905090036002900360015500");
        let balance = U256::from(0x3635c9adc5dea00000u128);
        let code_hash = keccak256(code);
        db_tx
            .put::<tables::PlainAccountState>(
                acc1,
                Account { nonce: 0, balance: U256::ZERO, bytecode_hash: Some(code_hash) },
            )
            .unwrap();
        db_tx
            .put::<tables::PlainAccountState>(
                acc2,
                Account { nonce: 0, balance, bytecode_hash: None },
            )
            .unwrap();
        db_tx.put::<tables::Bytecodes>(code_hash, Bytecode::new_raw(code.to_vec().into())).unwrap();
        tx.commit().unwrap();

        let mut execution_stage = stage();
        let output = execution_stage.execute(&mut tx, input).await.unwrap();
        tx.commit().unwrap();
        assert_eq!(output, ExecOutput { stage_progress: 1, done: true });
        let tx = tx.deref_mut();
        // check post state
        let account1 = H160(hex!("1000000000000000000000000000000000000000"));
        let account1_info =
            Account { balance: U256::ZERO, nonce: 0x00, bytecode_hash: Some(code_hash) };
        let account2 = H160(hex!("2adc25665018aa1fe0e6bc666dac8fc2697ff9ba"));
        let account2_info = Account {
            balance: U256::from(0x1bc16d674ece94bau128),
            nonce: 0x00,
            bytecode_hash: None,
        };
        let account3 = H160(hex!("a94f5374fce5edbc8e2a8697c15331677e6ebf0b"));
        let account3_info = Account {
            balance: U256::from(0x3635c9adc5de996b46u128),
            nonce: 0x01,
            bytecode_hash: None,
        };

        // assert accounts
        assert_eq!(
            tx.get::<tables::PlainAccountState>(account1),
            Ok(Some(account1_info)),
            "Post changed of a account"
        );
        assert_eq!(
            tx.get::<tables::PlainAccountState>(account2),
            Ok(Some(account2_info)),
            "Post changed of a account"
        );
        assert_eq!(
            tx.get::<tables::PlainAccountState>(account3),
            Ok(Some(account3_info)),
            "Post changed of a account"
        );
        // assert storage
        // Get on dupsort would return only first value. This is good enough for this test.
        assert_eq!(
            tx.get::<tables::PlainStorageState>(account1),
            Ok(Some(StorageEntry { key: H256::from_low_u64_be(1), value: U256::from(2) })),
            "Post changed of a account"
        );
    }

    #[tokio::test]
    async fn sanity_execute_unwind() {
        // TODO cleanup the setup after https://github.com/paradigmxyz/reth/issues/332
        // is merged as it has similar framework

        let state_db = create_test_db::<WriteMap>(EnvKind::RW);
        let mut tx = Transaction::new(state_db.as_ref()).unwrap();
        let input = ExecInput {
            previous_stage: Some((PREV_STAGE_ID, 1)),
            /// The progress of this stage the last time it was executed.
            stage_progress: None,
        };
        let mut genesis_rlp = hex!("f901faf901f5a00000000000000000000000000000000000000000000000000000000000000000a01dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347942adc25665018aa1fe0e6bc666dac8fc2697ff9baa045571b40ae66ca7480791bbb2887286e4e4c4b1b298b191c889d6959023a32eda056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421b901000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000083020000808502540be400808000a00000000000000000000000000000000000000000000000000000000000000000880000000000000000c0c0").as_slice();
        let genesis = SealedBlock::decode(&mut genesis_rlp).unwrap();
        let mut block_rlp = hex!("f90262f901f9a075c371ba45999d87f4542326910a11af515897aebce5265d3f6acd1f1161f82fa01dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347942adc25665018aa1fe0e6bc666dac8fc2697ff9baa098f2dcd87c8ae4083e7017a05456c14eea4b1db2032126e27b3b1563d57d7cc0a08151d548273f6683169524b66ca9fe338b9ce42bc3540046c828fd939ae23bcba03f4e5c2ec5b2170b711d97ee755c160457bb58d8daa338e835ec02ae6860bbabb901000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000083020000018502540be40082a8798203e800a00000000000000000000000000000000000000000000000000000000000000000880000000000000000f863f861800a8405f5e10094100000000000000000000000000000000000000080801ba07e09e26678ed4fac08a249ebe8ed680bf9051a5e14ad223e4b2b9d26e0208f37a05f6e3f188e3e6eab7d7d3b6568f5eac7d687b08d307d3154ccd8c87b4630509bc0").as_slice();
        let block = SealedBlock::decode(&mut block_rlp).unwrap();
        insert_canonical_block(tx.deref_mut(), genesis, None, true).unwrap();
        insert_canonical_block(tx.deref_mut(), block.clone(), None, true).unwrap();
        tx.commit().unwrap();

        // variables
        let code = hex!("5a465a905090036002900360015500");
        let balance = U256::from(0x3635c9adc5dea00000u128);
        let code_hash = keccak256(code);
        // pre state
        let db_tx = tx.deref_mut();
        let acc1 = H160(hex!("1000000000000000000000000000000000000000"));
        let acc1_info = Account { nonce: 0, balance: U256::ZERO, bytecode_hash: Some(code_hash) };
        let acc2 = H160(hex!("a94f5374fce5edbc8e2a8697c15331677e6ebf0b"));
        let acc2_info = Account { nonce: 0, balance, bytecode_hash: None };

        db_tx.put::<tables::PlainAccountState>(acc1, acc1_info).unwrap();
        db_tx.put::<tables::PlainAccountState>(acc2, acc2_info).unwrap();
        db_tx.put::<tables::Bytecodes>(code_hash, Bytecode::new_raw(code.to_vec().into())).unwrap();
        tx.commit().unwrap();

        // execute
        let mut execution_stage = stage();
        let _ = execution_stage.execute(&mut tx, input).await.unwrap();
        tx.commit().unwrap();

        let mut stage = stage();
        let o = stage
            .unwind(&mut tx, UnwindInput { stage_progress: 1, unwind_to: 0, bad_block: None })
            .await
            .unwrap();

        assert_eq!(o, UnwindOutput { stage_progress: 0 });

        // assert unwind stage
        let db_tx = tx.deref();
        assert_eq!(
            db_tx.get::<tables::PlainAccountState>(acc1),
            Ok(Some(acc1_info)),
            "Pre changed of a account"
        );
        assert_eq!(
            db_tx.get::<tables::PlainAccountState>(acc2),
            Ok(Some(acc2_info)),
            "Post changed of a account"
        );

        let miner_acc = H160(hex!("2adc25665018aa1fe0e6bc666dac8fc2697ff9ba"));
        assert_eq!(
            db_tx.get::<tables::PlainAccountState>(miner_acc),
            Ok(None),
            "Third account should be unwinded"
        );
    }

    #[tokio::test]
    async fn test_selfdestruct() {
        let test_tx = TestTransaction::default();
        let mut tx = test_tx.inner();
        let input = ExecInput {
            previous_stage: Some((PREV_STAGE_ID, 1)),
            /// The progress of this stage the last time it was executed.
            stage_progress: None,
        };
        let mut genesis_rlp = hex!("f901f8f901f3a00000000000000000000000000000000000000000000000000000000000000000a01dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347942adc25665018aa1fe0e6bc666dac8fc2697ff9baa0c9ceb8372c88cb461724d8d3d87e8b933f6fc5f679d4841800e662f4428ffd0da056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421b90100000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000008302000080830f4240808000a00000000000000000000000000000000000000000000000000000000000000000880000000000000000c0c0").as_slice();
        let genesis = SealedBlock::decode(&mut genesis_rlp).unwrap();
        let mut block_rlp = hex!("f9025ff901f7a0c86e8cc0310ae7c531c758678ddbfd16fc51c8cef8cec650b032de9869e8b94fa01dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347942adc25665018aa1fe0e6bc666dac8fc2697ff9baa050554882fbbda2c2fd93fdc466db9946ea262a67f7a76cc169e714f105ab583da00967f09ef1dfed20c0eacfaa94d5cd4002eda3242ac47eae68972d07b106d192a0e3c8b47fbfc94667ef4cceb17e5cc21e3b1eebd442cebb27f07562b33836290db90100000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000008302000001830f42408238108203e800a00000000000000000000000000000000000000000000000000000000000000000880000000000000000f862f860800a83061a8094095e7baea6a6c7c4c2dfeb977efac326af552d8780801ba072ed817487b84ba367d15d2f039b5fc5f087d0a8882fbdf73e8cb49357e1ce30a0403d800545b8fc544f92ce8124e2255f8c3c6af93f28243a120585d4c4c6a2a3c0").as_slice();
        let block = SealedBlock::decode(&mut block_rlp).unwrap();
        insert_canonical_block(tx.deref_mut(), genesis, None, true).unwrap();
        insert_canonical_block(tx.deref_mut(), block.clone(), None, true).unwrap();
        tx.commit().unwrap();

        // variables
        let caller_address = H160(hex!("a94f5374fce5edbc8e2a8697c15331677e6ebf0b"));
        let destroyed_address = H160(hex!("095e7baea6a6c7c4c2dfeb977efac326af552d87"));
        let beneficiary_address = H160(hex!("2adc25665018aa1fe0e6bc666dac8fc2697ff9ba"));

        let code = hex!("73095e7baea6a6c7c4c2dfeb977efac326af552d8731ff00");
        let balance = U256::from(0x0de0b6b3a7640000u64);
        let code_hash = keccak256(code);

        // pre state
        let db_tx = tx.deref_mut();
        let caller_info = Account { nonce: 0, balance, bytecode_hash: None };
        let destroyed_info =
            Account { nonce: 0, balance: U256::ZERO, bytecode_hash: Some(code_hash) };

        // set account
        db_tx.put::<tables::PlainAccountState>(caller_address, caller_info).unwrap();
        db_tx.put::<tables::PlainAccountState>(destroyed_address, destroyed_info).unwrap();
        db_tx.put::<tables::Bytecodes>(code_hash, Bytecode::new_raw(code.to_vec().into())).unwrap();
        // set storage to check when account gets destroyed.
        db_tx
            .put::<tables::PlainStorageState>(
                destroyed_address,
                StorageEntry { key: H256::zero(), value: U256::ZERO },
            )
            .unwrap();
        db_tx
            .put::<tables::PlainStorageState>(
                destroyed_address,
                StorageEntry { key: H256::from_low_u64_be(1), value: U256::from(1u64) },
            )
            .unwrap();

        tx.commit().unwrap();

        // execute
        let mut execution_stage = stage();
        let _ = execution_stage.execute(&mut tx, input).await.unwrap();
        tx.commit().unwrap();

        // assert unwind stage
        assert_eq!(
            tx.deref().get::<tables::PlainAccountState>(destroyed_address),
            Ok(None),
            "Account was destroyed"
        );

        assert_eq!(
            tx.deref().get::<tables::PlainStorageState>(destroyed_address),
            Ok(None),
            "There is storage for destroyed account"
        );
        // drops tx so that it returns write privilege to test_tx
        drop(tx);
        let plain_accounts = test_tx.table::<tables::PlainAccountState>().unwrap();
        let plain_storage = test_tx.table::<tables::PlainStorageState>().unwrap();

        assert_eq!(
            plain_accounts,
            vec![
                (
                    beneficiary_address,
                    Account {
                        nonce: 0,
                        balance: U256::from(0x1bc16d674eca30a0u64),
                        bytecode_hash: None
                    }
                ),
                (
                    caller_address,
                    Account {
                        nonce: 1,
                        balance: U256::from(0xde0b6b3a761cf60u64),
                        bytecode_hash: None
                    }
                )
            ]
        );
        assert!(plain_storage.is_empty());

        let account_changesets = test_tx.table::<tables::AccountChangeSet>().unwrap();
        let storage_changesets = test_tx.table::<tables::StorageChangeSet>().unwrap();

        assert_eq!(
            account_changesets,
            vec![
                (1, AccountBeforeTx { address: destroyed_address, info: Some(destroyed_info) }),
                (1, AccountBeforeTx { address: beneficiary_address, info: None }),
                (1, AccountBeforeTx { address: caller_address, info: Some(caller_info) }),
                (
                    2,
                    AccountBeforeTx {
                        address: beneficiary_address,
                        info: Some(Account {
                            nonce: 0,
                            balance: U256::from(0x230a0),
                            bytecode_hash: None
                        })
                    }
                )
            ]
        );

        assert_eq!(
            storage_changesets,
            vec![
                (
                    (1, destroyed_address).into(),
                    StorageEntry { key: H256::zero(), value: U256::ZERO }
                ),
                (
                    (1, destroyed_address).into(),
                    StorageEntry { key: H256::from_low_u64_be(1), value: U256::from(1u64) }
                )
            ]
        );
    }
}
