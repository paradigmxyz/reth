use crate::{
    db::Transaction, trie::DBTrieLoader, ExecInput, ExecOutput, Stage, StageError, StageId,
    UnwindInput, UnwindOutput,
};
use reth_db::{database::Database, tables, transaction::DbTxMut};
use reth_interfaces::consensus;
use std::fmt::Debug;
use tracing::*;

const MERKLE_EXECUTION: StageId = StageId("MerkleExecuteStage");
const MERKLE_UNWIND: StageId = StageId("MerkleUnwindStage");

/// Merkle stage uses input from [AccountHashingStage] and [StorageHashingStage] stages
/// and calculated intermediate hashed and state root.
/// This stage depends on the Account and Storage stages. It will be executed after them during
/// execution, and before them during unwinding.
#[derive(Debug)]
pub struct MerkleStage {
    /// Flag if true would do `execute` but skip unwind but if it false it would skip execution but
    /// do unwind.
    pub is_execute: bool,
    /// The threshold for switching from incremental trie building
    /// of changes to whole rebuild. Num of transitions.
    pub clean_threshold: u64,
}

#[async_trait::async_trait]
impl<DB: Database> Stage<DB> for MerkleStage {
    /// Return the id of the stage
    fn id(&self) -> StageId {
        if self.is_execute {
            MERKLE_EXECUTION
        } else {
            MERKLE_UNWIND
        }
    }

    /// Execute the stage.
    async fn execute(
        &mut self,
        tx: &mut Transaction<'_, DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        if !self.is_execute {
            info!(target: "sync::stages::merkle::unwind", "Stage is always skipped");
            return Ok(ExecOutput { stage_progress: input.previous_stage_progress(), done: true })
        }

        let stage_progress = input.stage_progress.unwrap_or_default();
        let previous_stage_progress = input.previous_stage_progress();

        let from_transition = tx.get_block_transition(stage_progress)?;
        let to_transition = tx.get_block_transition(previous_stage_progress)?;

        let block_root = tx.get_header_by_num(previous_stage_progress)?.state_root;

        let trie_root = if from_transition == to_transition {
            block_root
        } else if to_transition - from_transition > self.clean_threshold || stage_progress == 0 {
            debug!(target: "sync::stages::merkle::exec", current = ?stage_progress, target = ?previous_stage_progress, "Rebuilding trie");
            // if there are more blocks than threshold it is faster to rebuild the trie
            tx.clear::<tables::AccountsTrie>()?;
            tx.clear::<tables::StoragesTrie>()?;

            let loader = DBTrieLoader::default();
            loader.calculate_root(tx).map_err(|e| StageError::Fatal(Box::new(e)))?
        } else {
            debug!(target: "sync::stages::merkle::exec", current = ?stage_progress, target = ?previous_stage_progress, "Updating trie");
            // Iterate over changeset (similar to Hashing stages) and take new values
            let current_root = tx.get_header_by_num(stage_progress)?.state_root;
            let loader = DBTrieLoader::default();
            loader
                .update_root(tx, current_root, from_transition, to_transition)
                .map_err(|e| StageError::Fatal(Box::new(e)))?
        };

        if block_root != trie_root {
            warn!(target: "sync::stages::merkle::exec", ?previous_stage_progress, got = ?block_root, expected = ?trie_root, "Block's root state failed verification");
            return Err(StageError::Validation {
                block: previous_stage_progress,
                error: consensus::Error::BodyStateRootDiff { got: block_root, expected: trie_root },
            })
        }

        info!(target: "sync::stages::merkle::exec", "Stage finished");
        Ok(ExecOutput { stage_progress: input.previous_stage_progress(), done: true })
    }

    /// Unwind the stage.
    async fn unwind(
        &mut self,
        _tx: &mut Transaction<'_, DB>,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        if self.is_execute {
            info!(target: "sync::stages::merkle::exec", "Stage is always skipped");
            return Ok(UnwindOutput { stage_progress: input.unwind_to })
        }

        info!(target: "sync::stages::merkle::unwind", "Stage finished");
        Ok(UnwindOutput { stage_progress: input.unwind_to })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_utils::{
            stage_test_suite_ext, ExecuteStageTestRunner, StageTestRunner, TestRunnerError,
            TestTransaction, UnwindStageTestRunner, PREV_STAGE_ID,
        },
        trie::EthAccount,
    };
    use assert_matches::assert_matches;
    use reth_db::{
        cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW},
        models::{AccountBeforeTx, BlockNumHash, StoredBlockBody},
        transaction::DbTx,
    };
    use reth_interfaces::test_utils::generators::{
        random_block, random_block_range, random_contract_account_range,
    };
    use reth_primitives::{
        keccak256, proofs::KeccakHasher, Account, Address, SealedBlock, StorageEntry, H256, U256,
    };
    use reth_rlp::Encodable;
    use std::collections::BTreeMap;
    use triehash::sec_trie_root;

    stage_test_suite_ext!(MerkleTestRunner, merkle);

    /// Execute with low clean threshold so as to hash whole storage
    #[tokio::test]
    async fn execute_clean_merkle() {
        let (previous_stage, stage_progress) = (500, 0);

        // Set up the runner
        let mut runner = MerkleTestRunner::default();
        // set low threshold so we hash the whole storage
        runner.set_clean_threshold(1);
        let input = ExecInput {
            previous_stage: Some((PREV_STAGE_ID, previous_stage)),
            stage_progress: Some(stage_progress),
        };

        runner.seed_execution(input).expect("failed to seed execution");

        let rx = runner.execute(input);

        // Assert the successful result
        let result = rx.await.unwrap();
        assert_matches!(
            result,
            Ok(ExecOutput { done, stage_progress })
                if done && stage_progress == previous_stage
        );

        // Validate the stage execution
        assert!(runner.validate_execution(input, result.ok()).is_ok(), "execution validation");
    }

    struct MerkleTestRunner {
        tx: TestTransaction,
        clean_threshold: u64,
        is_execute: bool,
    }

    impl Default for MerkleTestRunner {
        fn default() -> Self {
            Self { tx: TestTransaction::default(), clean_threshold: 100000, is_execute: true }
        }
    }

    impl StageTestRunner for MerkleTestRunner {
        type S = MerkleStage;

        fn tx(&self) -> &TestTransaction {
            &self.tx
        }

        fn stage(&self) -> Self::S {
            Self::S { clean_threshold: self.clean_threshold, is_execute: self.is_execute }
        }
    }

    #[async_trait::async_trait]
    impl ExecuteStageTestRunner for MerkleTestRunner {
        type Seed = Vec<SealedBlock>;

        fn seed_execution(&mut self, input: ExecInput) -> Result<Self::Seed, TestRunnerError> {
            let stage_progress = input.stage_progress.unwrap_or_default();
            let end = input.previous_stage_progress() + 1;

            let n_accounts = 11;
            let mut accounts = random_contract_account_range(&mut (0..n_accounts));

            let SealedBlock { header, body, ommers } =
                random_block(stage_progress, None, None, None);
            let mut header = header.unseal();
            header.state_root = self.generate_initial_trie(&accounts)?;
            let sealed_head = SealedBlock { header: header.seal(), body, ommers };

            let head_hash = sealed_head.hash();
            let mut blocks = vec![sealed_head];

            blocks.extend(random_block_range((stage_progress + 1)..end, head_hash, 0..2));

            self.tx.insert_headers(blocks.iter().map(|block| &block.header))?;

            let (mut transition_id, mut tx_id) = (0, 0);

            let mut storages: BTreeMap<Address, BTreeMap<H256, U256>> = BTreeMap::new();

            for progress in blocks.iter() {
                // Insert last progress data
                self.tx.commit(|tx| {
                    let key: BlockNumHash = (progress.number, progress.hash()).into();

                    let body = StoredBlockBody {
                        start_tx_id: tx_id,
                        tx_count: progress.body.len() as u64,
                    };

                    progress.body.iter().try_for_each(|transaction| {
                        tx.put::<tables::TxHashNumber>(transaction.hash(), tx_id)?;
                        tx.put::<tables::Transactions>(tx_id, transaction.clone())?;
                        tx.put::<tables::TxTransitionIndex>(tx_id, transition_id)?;
                        tx_id += 1;
                        Ok(())
                    })?;

                    // seed account changeset
                    for (addr, prev_acc) in &mut accounts {
                        let acc_before_tx =
                            AccountBeforeTx { address: *addr, info: Some(*prev_acc) };
                        transition_id += 1;
                        tx.put::<tables::AccountChangeSet>(transition_id, acc_before_tx)?;

                        prev_acc.nonce += 1;
                        prev_acc.balance = prev_acc.balance.wrapping_add(U256::from(1));

                        let new_entry = StorageEntry {
                            key: keccak256([rand::random::<u8>()]),
                            value: U256::from(rand::random::<u8>()),
                        };
                        let storage = storages.entry(*addr).or_default();
                        let old_value = storage.entry(new_entry.key).or_default();

                        transition_id += 1;
                        tx.put::<tables::StorageChangeSet>(
                            (transition_id, *addr).into(),
                            StorageEntry { key: new_entry.key, value: *old_value },
                        )?;

                        *old_value = new_entry.value;
                    }

                    tx.put::<tables::BlockTransitionIndex>(key.number(), transition_id)?;
                    tx.put::<tables::BlockBodies>(key, body)
                })?;
            }

            self.insert_accounts(&accounts)?;
            self.insert_storages(&storages)?;

            let last_numhash = self.tx.inner().get_block_numhash(end - 1).unwrap();
            let root = self.state_root()?;
            self.tx.commit(|tx| {
                let mut last_header = tx.get::<tables::Headers>(last_numhash)?.unwrap();
                last_header.state_root = root;
                tx.put::<tables::Headers>(last_numhash, last_header)
            })?;

            Ok(blocks)
        }

        fn validate_execution(
            &self,
            input: ExecInput,
            output: Option<ExecOutput>,
        ) -> Result<(), TestRunnerError> {
            if let Some(output) = output {
                let start_block = input.stage_progress.unwrap_or_default() + 1;
                let end_block = output.stage_progress;
                if start_block > end_block {
                    return Ok(())
                }
            }
            self.check_table_is_populated()
        }
    }

    impl UnwindStageTestRunner for MerkleTestRunner {
        fn validate_unwind(&self, input: UnwindInput) -> Result<(), TestRunnerError> {
            self.unwind_storage(input)?;
            self.check_table_is_populated()
        }
    }

    impl MerkleTestRunner {
        #[allow(dead_code)]
        fn set_clean_threshold(&mut self, threshold: u64) {
            self.clean_threshold = threshold;
        }

        #[allow(dead_code)]
        fn set_is_execute(&mut self, is_execute: bool) {
            self.is_execute = is_execute;
        }

        fn state_root(&self) -> Result<H256, TestRunnerError> {
            self.tx
                .query(|tx| {
                    let mut accounts_cursor = tx.cursor_read::<tables::PlainAccountState>()?;
                    let mut storage_cursor = tx.cursor_dup_read::<tables::PlainStorageState>()?;
                    // TODO: maybe extract root calculation as a test util?
                    let accounts = accounts_cursor.walk(Address::zero())?.map_while(|res| {
                        let Ok((address, account)) = res else {
                                return None
                            };
                        let mut bytes = Vec::new();
                        let storage = storage_cursor
                            .walk_dup(address, H256::zero())
                            .unwrap()
                            .map_while(|res| {
                                let Ok((_, StorageEntry { key, value })) = res else { return None };
                                let mut bytes = Vec::new();
                                value.encode(&mut bytes);
                                Some((key, bytes))
                            });
                        let root = sec_trie_root::<KeccakHasher, _, _, _>(storage);
                        let eth_account = EthAccount::from_with_root(account, root);
                        eth_account.encode(&mut bytes);
                        Some((address, bytes))
                    });
                    Ok(sec_trie_root::<KeccakHasher, _, _, _>(accounts))
                })
                .map_err(|e| e.into())
        }

        pub(crate) fn generate_initial_trie(
            &self,
            accounts: &[(Address, Account)],
        ) -> Result<H256, TestRunnerError> {
            self.insert_accounts(accounts)?;

            let loader = DBTrieLoader::default();

            let mut tx = self.tx.inner();
            let root = loader.calculate_root(&tx).expect("couldn't create initial trie");

            tx.commit()?;

            Ok(root)
        }

        pub(crate) fn insert_accounts(
            &self,
            accounts: &[(Address, Account)],
        ) -> Result<(), TestRunnerError> {
            for (addr, acc) in accounts.iter() {
                self.tx.commit(|tx| {
                    tx.put::<tables::PlainAccountState>(*addr, *acc)?;
                    tx.put::<tables::HashedAccount>(keccak256(addr), *acc)?;
                    Ok(())
                })?;
            }

            Ok(())
        }

        fn insert_storages(
            &self,
            storages: &BTreeMap<Address, BTreeMap<H256, U256>>,
        ) -> Result<(), TestRunnerError> {
            self.tx
                .commit(|tx| {
                    storages.iter().try_for_each(|(&addr, storage)| {
                        storage.iter().try_for_each(|(&key, &value)| {
                            let entry = StorageEntry { key, value };
                            tx.put::<tables::PlainStorageState>(addr, entry)
                        })
                    })?;
                    storages
                        .iter()
                        .map(|(addr, storage)| {
                            (
                                keccak256(addr),
                                storage.iter().map(|(key, value)| (keccak256(key), value)),
                            )
                        })
                        .collect::<BTreeMap<_, _>>()
                        .into_iter()
                        .try_for_each(|(addr, storage)| {
                            storage.into_iter().try_for_each(|(key, &value)| {
                                let entry = StorageEntry { key, value };
                                tx.put::<tables::HashedStorage>(addr, entry)
                            })
                        })?;
                    Ok(())
                })
                .map_err(|e| e.into())
        }

        fn unwind_storage(&self, input: UnwindInput) -> Result<(), TestRunnerError> {
            let target_transition = self
                .tx
                .inner()
                .get_block_transition(input.unwind_to)
                .map_err(|e| TestRunnerError::Internal(Box::new(e)))?;
            self.tx.commit(|tx| {
                let mut storage_cursor = tx.cursor_dup_write::<tables::PlainStorageState>()?;
                let mut changeset_cursor = tx.cursor_dup_read::<tables::StorageChangeSet>()?;

                let mut rev_changeset_walker = changeset_cursor.walk_back(None)?;

                while let Some((tid_address, entry)) = rev_changeset_walker.next().transpose()? {
                    if tid_address.transition_id() <= target_transition {
                        break
                    }

                    storage_cursor.seek_by_key_subkey(tid_address.address(), entry.key)?;
                    storage_cursor.delete_current()?;

                    if entry.value != U256::ZERO {
                        storage_cursor.append_dup(tid_address.address(), entry)?;
                    }
                }
                Ok(())
            })?;
            Ok(())
        }

        fn check_table_is_populated(&self) -> Result<(), TestRunnerError> {
            assert_eq!(
                self.tx.table_is_empty::<tables::AccountsTrie>()?,
                self.tx.table_is_empty::<tables::PlainAccountState>()?
            );
            assert_eq!(
                self.tx.table_is_empty::<tables::StoragesTrie>()?,
                self.tx.table_is_empty::<tables::PlainStorageState>()?
            );
            Ok(())
        }
    }
}
