use crate::{
    db::Transaction, ExecInput, ExecOutput, Stage, StageError, StageId, UnwindInput, UnwindOutput,
};
use reth_db::{
    cursor::{DbCursorRO, DbCursorRW},
    database::Database,
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_primitives::{keccak256, Account, Address, H160};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Debug,
};
use tracing::*;

/// The [`StageId`] of the account hashing stage.
pub const ACCOUNT_HASHING: StageId = StageId("AccountHashing");

/// Account hashing stage hashes plain account.
/// This is preparation before generating intermediate hashes and calculating Merkle tree root.
#[derive(Debug)]
pub struct AccountHashingStage {
    /// The threshold (in number of state transitions) for switching between incremental
    /// hashing and full storage hashing.
    pub clean_threshold: u64,
    /// The maximum number of blocks to process before committing.
    pub commit_threshold: u64,
}

impl Default for AccountHashingStage {
    fn default() -> Self {
        Self { clean_threshold: 500_000, commit_threshold: 100_000 }
    }
}

#[async_trait::async_trait]
impl<DB: Database> Stage<DB> for AccountHashingStage {
    /// Return the id of the stage
    fn id(&self) -> StageId {
        ACCOUNT_HASHING
    }

    /// Execute the stage.
    async fn execute(
        &mut self,
        tx: &mut Transaction<'_, DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        let stage_progress = input.stage_progress.unwrap_or_default();
        let previous_stage_progress = input.previous_stage_progress();

        // read account changeset, merge it into one changeset and calculate account hashes.
        let from_transition = tx.get_block_transition(stage_progress)?;
        let to_transition = tx.get_block_transition(previous_stage_progress)?;

        // if there are more blocks then threshold it is faster to go over Plain state and hash all
        // account otherwise take changesets aggregate the sets and apply hashing to
        // AccountHashing table. Also, if we start from genesis, we need to hash from scratch, as
        // genesis accounts are not in changeset.
        if to_transition - from_transition > self.clean_threshold || stage_progress == 0 {
            // clear table, load all accounts and hash it
            tx.clear::<tables::HashedAccount>()?;
            tx.commit()?;

            let mut first_key = H160::zero();
            loop {
                let next_key = {
                    let mut accounts = tx.cursor_read::<tables::PlainAccountState>()?;

                    let hashed_batch = accounts
                        .walk(first_key)?
                        .take(self.commit_threshold as usize)
                        .map(|res| res.map(|(address, account)| (keccak256(address), account)))
                        .collect::<Result<BTreeMap<_, _>, _>>()?;

                    // next key of iterator
                    let next_key = accounts.next()?;

                    // iterate and put presorted hashed accounts
                    hashed_batch
                        .into_iter()
                        .try_for_each(|(k, v)| tx.put::<tables::HashedAccount>(k, v))?;
                    next_key
                };
                tx.commit()?;
                if let Some((next_key, _)) = next_key {
                    first_key = next_key;
                    continue
                }
                break
            }
        } else {
            let mut plain_accounts = tx.cursor_read::<tables::PlainAccountState>()?;
            let mut hashed_accounts = tx.cursor_write::<tables::HashedAccount>()?;

            // Aggregate all transition changesets and and make list of account that have been
            // changed.
            tx.cursor_read::<tables::AccountChangeSet>()?
                .walk_range(from_transition..to_transition)?
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                // fold all account to one set of changed accounts
                .fold(BTreeSet::new(), |mut accounts: BTreeSet<Address>, (_, account_before)| {
                    accounts.insert(account_before.address);
                    accounts
                })
                .into_iter()
                // iterate over plain state and get newest value.
                // Assumption we are okay to make is that plainstate represent
                // `previous_stage_progress` state.
                .map(|address| {
                    plain_accounts.seek_exact(address).map(|a| (address, a.map(|(_, v)| v)))
                })
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .try_for_each(|(address, account)| -> Result<(), StageError> {
                    let hashed_address = keccak256(address);
                    if let Some(account) = account {
                        hashed_accounts.upsert(hashed_address, account)?
                    } else if hashed_accounts.seek_exact(hashed_address)?.is_some() {
                        hashed_accounts.delete_current()?;
                    }
                    Ok(())
                })?;
        }

        info!(target: "sync::stages::hashing_account", "Stage finished");
        Ok(ExecOutput { stage_progress: input.previous_stage_progress(), done: true })
    }

    /// Unwind the stage.
    async fn unwind(
        &mut self,
        tx: &mut Transaction<'_, DB>,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        // There is no threshold on account unwind, we will always take changesets and
        // apply past values to HashedAccount table.

        let from_transition_rev = tx.get_block_transition(input.unwind_to)?;
        let to_transition_rev = tx.get_block_transition(input.stage_progress)?;

        let mut hashed_accounts = tx.cursor_write::<tables::HashedAccount>()?;

        // Aggregate all transition changesets and and make list of account that have been changed.
        tx.cursor_read::<tables::AccountChangeSet>()?
            .walk_range(from_transition_rev..to_transition_rev)?
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .rev()
            // fold all account to get the old balance/nonces and account that needs to be removed
            .fold(
                BTreeMap::new(),
                |mut accounts: BTreeMap<Address, Option<Account>>, (_, account_before)| {
                    accounts.insert(account_before.address, account_before.info);
                    accounts
                },
            )
            .into_iter()
            // hash addresses and collect it inside sorted BTreeMap.
            // We are doing keccak only once per address.
            .map(|(address, account)| (keccak256(address), account))
            .collect::<BTreeMap<_, _>>()
            .into_iter()
            // Apply values to HashedState (if Account is None remove it);
            .try_for_each(|(hashed_address, account)| -> Result<(), StageError> {
                if let Some(account) = account {
                    hashed_accounts.upsert(hashed_address, account)?;
                } else if hashed_accounts.seek_exact(hashed_address)?.is_some() {
                    hashed_accounts.delete_current()?;
                }
                Ok(())
            })?;

        Ok(UnwindOutput { stage_progress: input.unwind_to })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{
        stage_test_suite_ext, ExecuteStageTestRunner, TestRunnerError, UnwindStageTestRunner,
        PREV_STAGE_ID,
    };
    use assert_matches::assert_matches;
    use reth_interfaces::test_utils::generators::random_block_range;
    use reth_primitives::{Account, SealedBlock, H256, U256};
    use reth_provider::insert_canonical_block;
    use test_utils::*;

    stage_test_suite_ext!(AccountHashingTestRunner, account_hashing);

    #[tokio::test]
    async fn execute_clean_account_hashing() {
        let (previous_stage, stage_progress) = (20, 10);
        // Set up the runner
        let mut runner = AccountHashingTestRunner::default();
        runner.set_clean_threshold(1);

        let input = ExecInput {
            previous_stage: Some((PREV_STAGE_ID, previous_stage)),
            stage_progress: Some(stage_progress),
        };

        runner.seed_execution(input).expect("failed to seed execution");

        let rx = runner.execute(input);
        let result = rx.await.unwrap();

        assert_matches!(result, Ok(ExecOutput {done, stage_progress}) if done && stage_progress == previous_stage);

        // Validate the stage execution
        assert!(runner.validate_execution(input, result.ok()).is_ok(), "execution validation");
    }

    mod test_utils {
        use super::*;
        use crate::{
            stages::hashing_account::AccountHashingStage,
            test_utils::{StageTestRunner, TestTransaction},
            ExecInput, ExecOutput, UnwindInput,
        };
        use reth_db::{
            cursor::DbCursorRO,
            models::AccountBeforeTx,
            tables,
            transaction::{DbTx, DbTxMut},
        };
        use reth_interfaces::test_utils::generators::random_eoa_account_range;

        pub(crate) struct AccountHashingTestRunner {
            pub(crate) tx: TestTransaction,
            commit_threshold: u64,
            clean_threshold: u64,
        }

        impl AccountHashingTestRunner {
            pub(crate) fn set_clean_threshold(&mut self, threshold: u64) {
                self.clean_threshold = threshold;
            }

            #[allow(dead_code)]
            pub(crate) fn set_commit_threshold(&mut self, threshold: u64) {
                self.commit_threshold = threshold;
            }

            pub(crate) fn insert_blocks(
                &self,
                blocks: Vec<SealedBlock>,
            ) -> Result<(), TestRunnerError> {
                for block in blocks.iter() {
                    self.tx.commit(|tx| {
                        insert_canonical_block(tx, block, true).unwrap();
                        Ok(())
                    })?;
                }

                Ok(())
            }

            pub(crate) fn insert_accounts(
                &self,
                accounts: &[(Address, Account)],
            ) -> Result<(), TestRunnerError> {
                for (addr, acc) in accounts.iter() {
                    self.tx.commit(|tx| {
                        tx.put::<tables::PlainAccountState>(*addr, *acc)?;
                        Ok(())
                    })?;
                }

                Ok(())
            }

            /// Iterates over PlainAccount table and checks that the accounts match the ones
            /// in the HashedAccount table
            pub(crate) fn check_hashed_accounts(&self) -> Result<(), TestRunnerError> {
                self.tx.query(|tx| {
                    let mut acc_cursor = tx.cursor_read::<tables::PlainAccountState>()?;
                    let mut hashed_acc_cursor = tx.cursor_read::<tables::HashedAccount>()?;

                    while let Some((address, account)) = acc_cursor.next()? {
                        let hashed_addr = keccak256(address);
                        if let Some((_, acc)) = hashed_acc_cursor.seek_exact(hashed_addr)? {
                            assert_eq!(acc, account)
                        }
                    }
                    Ok(())
                })?;

                Ok(())
            }

            /// Same as check_hashed_accounts, only that checks with the old account state,
            /// namely, the same account with nonce - 1 and balance - 1.
            pub(crate) fn check_old_hashed_accounts(&self) -> Result<(), TestRunnerError> {
                self.tx.query(|tx| {
                    let mut acc_cursor = tx.cursor_read::<tables::PlainAccountState>()?;
                    let mut hashed_acc_cursor = tx.cursor_read::<tables::HashedAccount>()?;

                    while let Some((address, account)) = acc_cursor.next()? {
                        let Account { nonce, balance, .. } = account;
                        let old_acc = Account {
                            nonce: nonce - 1,
                            balance: balance - U256::from(1),
                            bytecode_hash: None,
                        };
                        let hashed_addr = keccak256(address);
                        if let Some((_, acc)) = hashed_acc_cursor.seek_exact(hashed_addr)? {
                            assert_eq!(acc, old_acc)
                        }
                    }
                    Ok(())
                })?;

                Ok(())
            }
        }

        impl Default for AccountHashingTestRunner {
            fn default() -> Self {
                Self {
                    tx: TestTransaction::default(),
                    commit_threshold: 1000,
                    clean_threshold: 1000,
                }
            }
        }

        impl StageTestRunner for AccountHashingTestRunner {
            type S = AccountHashingStage;

            fn tx(&self) -> &TestTransaction {
                &self.tx
            }

            fn stage(&self) -> Self::S {
                Self::S {
                    commit_threshold: self.commit_threshold,
                    clean_threshold: self.clean_threshold,
                }
            }
        }

        #[async_trait::async_trait]
        impl ExecuteStageTestRunner for AccountHashingTestRunner {
            type Seed = Vec<(Address, Account)>;

            fn seed_execution(&mut self, input: ExecInput) -> Result<Self::Seed, TestRunnerError> {
                let end = input.previous_stage_progress() + 1;

                let blocks = random_block_range(0..end, H256::zero(), 0..3);
                self.insert_blocks(blocks)?;

                let n_accounts = 2;
                let accounts = random_eoa_account_range(&mut (0..n_accounts));
                self.insert_accounts(&accounts)?;

                // seed account changeset
                self.tx
                    .commit(|tx| {
                        let (_, last_transition) =
                            tx.cursor_read::<tables::BlockTransitionIndex>()?.last()?.unwrap();

                        let first_transition =
                            last_transition.checked_sub(n_accounts).unwrap_or_default();

                        for (t, (addr, acc)) in (first_transition..last_transition).zip(&accounts) {
                            let Account { nonce, balance, .. } = acc;
                            let prev_acc = Account {
                                nonce: nonce - 1,
                                balance: balance - U256::from(1),
                                bytecode_hash: None,
                            };
                            let acc_before_tx =
                                AccountBeforeTx { address: *addr, info: Some(prev_acc) };
                            tx.put::<tables::AccountChangeSet>(t, acc_before_tx)?;
                        }

                        Ok(())
                    })
                    .unwrap();

                Ok(accounts)
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
                self.check_hashed_accounts()
            }
        }

        impl UnwindStageTestRunner for AccountHashingTestRunner {
            fn validate_unwind(&self, _input: UnwindInput) -> Result<(), TestRunnerError> {
                self.check_old_hashed_accounts()
            }
        }
    }
}
