use crate::{ExecInput, ExecOutput, Stage, StageError, StageId, UnwindInput, UnwindOutput};
use reth_db::{
    cursor::{DbCursorRO, DbCursorRW},
    database::Database,
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_primitives::{keccak256, Account, Address};
use reth_provider::Transaction;
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Debug,
    ops::Range,
};
use tracing::*;

/// The [`StageId`] of the account hashing stage.
pub const ACCOUNT_HASHING: StageId = StageId("AccountHashing");

/// Account hashing stage hashes plain account.
/// This is preparation before generating intermediate hashes and calculating Merkle tree root.
#[derive(Clone, Debug)]
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

#[derive(Clone, Debug)]
/// `SeedOpts` provides configuration parameters for calling `AccountHashingStage::seed`
/// in unit tests or benchmarks to generate an initial database state for running the
/// stage.
///
/// In order to check the "full hashing" mode of the stage you want to generate more
/// transitions than `AccountHashingStage.clean_threshold`. This requires:
/// 1. Creating enough blocks + transactions so there's enough transactions to generate
/// the required transition keys in the `BlockTransitionIndex` (which depends on the
/// `TxTransitionIndex` internally)
/// 2. Setting `transitions > clean_threshold` so that there's enough diffs to actually
/// take the 2nd codepath
pub struct SeedOpts {
    /// The range of blocks to be generated
    pub blocks: Range<u64>,
    /// The range of accounts to be generated
    pub accounts: Range<u64>,
    /// The range of transactions to be generated per block.
    pub txs: Range<u8>,
    /// The number of transitions to go back, capped at the number of total txs
    pub transitions: u64,
}

#[cfg(any(test, feature = "test-utils"))]
impl AccountHashingStage {
    /// Initializes the `PlainAccountState` table with `num_accounts` having some random state
    /// at the target block, with `txs_range` transactions in each block.
    ///
    /// Proceeds to go to the `BlockTransitionIndex` end, go back `transitions` and change the
    /// account state in the `AccountChangeSet` table.
    pub fn seed<DB: Database>(
        tx: &mut Transaction<'_, DB>,
        opts: SeedOpts,
    ) -> Result<Vec<(Address, Account)>, StageError> {
        use reth_db::models::AccountBeforeTx;
        use reth_interfaces::test_utils::generators::{
            random_block_range, random_eoa_account_range,
        };
        use reth_primitives::{H256, U256};
        use reth_provider::insert_canonical_block;

        let blocks = random_block_range(opts.blocks, H256::zero(), opts.txs);
        let num_transitions = blocks.iter().map(|b| b.body.len() as u64).sum();
        let transitions = std::cmp::min(opts.transitions, num_transitions);

        for block in blocks {
            insert_canonical_block(&**tx, &block, true).unwrap();
        }
        let mut accounts = random_eoa_account_range(opts.accounts);
        {
            // Account State generator
            let mut account_cursor = tx.cursor_write::<tables::PlainAccountState>()?;
            accounts.sort_by(|a, b| a.0.cmp(&b.0));
            for (addr, acc) in accounts.iter() {
                account_cursor.append(*addr, *acc)?;
            }

            // seed account changeset
            let (_, last_transition) =
                tx.cursor_read::<tables::BlockTransitionIndex>()?.last()?.unwrap();

            let first_transition = last_transition.checked_sub(transitions).unwrap_or_default();

            let mut acc_changeset_cursor = tx.cursor_write::<tables::AccountChangeSet>()?;
            for (t, (addr, acc)) in (first_transition..last_transition).zip(&accounts) {
                let Account { nonce, balance, .. } = acc;
                let prev_acc = Account {
                    nonce: nonce - 1,
                    balance: balance - U256::from(1),
                    bytecode_hash: None,
                };
                let acc_before_tx = AccountBeforeTx { address: *addr, info: Some(prev_acc) };
                acc_changeset_cursor.append(t, acc_before_tx)?;
            }
        }

        tx.commit()?;

        Ok(accounts)
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

            let mut first_key = None;
            loop {
                let next_key = {
                    let mut accounts = tx.cursor_read::<tables::PlainAccountState>()?;

                    let hashed_batch = accounts
                        .walk(first_key)?
                        .take(self.commit_threshold as usize)
                        .map(|res| res.map(|(address, account)| (keccak256(address), account)))
                        .collect::<Result<BTreeMap<_, _>, _>>()?;

                    let mut hashed_account_cursor = tx.cursor_write::<tables::HashedAccount>()?;

                    // iterate and put presorted hashed accounts
                    if first_key.is_none() {
                        hashed_batch
                            .into_iter()
                            .try_for_each(|(k, v)| hashed_account_cursor.append(k, v))?;
                    } else {
                        hashed_batch
                            .into_iter()
                            .try_for_each(|(k, v)| hashed_account_cursor.insert(k, v))?;
                    }

                    // next key of iterator
                    accounts.next()?
                };
                tx.commit()?;
                if let Some((next_key, _)) = next_key {
                    first_key = Some(next_key);
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
    use reth_primitives::{Account, U256};
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
        use reth_db::{cursor::DbCursorRO, tables, transaction::DbTx};

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
                Ok(AccountHashingStage::seed(
                    &mut self.tx.inner(),
                    SeedOpts {
                        blocks: 0..input.previous_stage_progress() + 1,
                        accounts: 0..2,
                        txs: 0..3,
                        transitions: 2,
                    },
                )
                .unwrap())
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
