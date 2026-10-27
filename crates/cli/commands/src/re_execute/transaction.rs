//! Ordered phase deltas shared by single-pass block and derived independent replay.

use super::chunk::{Chunk, Info, View};
use alloy_consensus::{transaction::TxHashRef, BlockHeader, TxReceipt};
use alloy_eip7928::bal::Bal;
use alloy_primitives::{Address, B256, U256};
use reth_cli_util::cancellation::CancellationToken;
use reth_consensus::FullConsensus;
use reth_evm::{
    block::{BlockExecutor, BlockExecutorFactory, TxResult},
    ConfigureEvm, Database, Evm,
};
use reth_primitives_traits::{Account, BlockBody, BlockTy, ReceiptTy, RecoveredBlock};
use reth_revm::{
    context::result::{ExecutionResult, Output},
    db::{states::bundle_state::BundleRetention, State},
    state::{bal::BlockAccessIndex, AccountInfo, Bytecode, EvmState, EvmStorageSlot},
    Database as _, DatabaseCommit,
};
use std::{collections::BTreeMap, sync::Arc, time::Instant};
use wincode::{SchemaRead, SchemaWrite};

/// Context and initial dependencies live once in the containing block record.
#[derive(Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub(super) struct Transaction {
    pub ordinal: u64,
    pub index: u64,
    pub hash: [u8; 32],
    pub before: Vec<u8>,
    pub after: Vec<u8>,
    pub outcome: Outcome,
    pub receipt: Vec<u8>,
    pub delta: Delta,
}

/// Preserve commit ordering inside a phase, including wipe then recreation.
#[derive(Debug, Default, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub(super) struct Delta(pub Vec<BTreeMap<[u8; 20], Change>>);

#[derive(Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub(super) struct Change {
    info: Option<InfoChange>,
    created: bool,
    pub(super) storage: BTreeMap<[u8; 32], [u8; 32]>,
    code: Option<Vec<u8>>,
}

/// None preserves a field; Some(zero/None) explicitly resets it.
#[derive(Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
struct InfoChange {
    nonce: Option<u64>,
    balance: Option<[u8; 32]>,
    code_hash: Option<Option<[u8; 32]>>,
}

#[derive(Debug, Default, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub(super) struct Phases {
    pub pre: Delta,
    pub post: Delta,
}

#[derive(Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub(super) struct Outcome {
    status: u8,
    reason: String,
    created: Option<[u8; 20]>,
    gas: [u64; 4],
    charged_gas: u64,
    output: Option<Vec<u8>>,
}

impl Outcome {
    fn capture<R: TxResult>(result: &R) -> Self
    where
        R::HaltReason: std::fmt::Debug,
    {
        let result = &result.result().result;
        let gas = result.gas();
        let (status, reason, created) = match result {
            ExecutionResult::Success { reason, output, .. } => (
                0,
                format!("{reason:?}"),
                match output {
                    Output::Create(_, a) => a.map(|a| a.into_array()),
                    _ => None,
                },
            ),
            ExecutionResult::Revert { .. } => (1, String::new(), None),
            ExecutionResult::Halt { reason, .. } => (2, format!("{reason:?}"), None),
        };
        Self {
            status,
            reason,
            created,
            gas: [
                gas.total_gas_spent(),
                gas.inner_refunded(),
                gas.floor_gas(),
                gas.state_gas_spent_final(),
            ],
            charged_gas: result.tx_gas_used(),
            output: result.output().map(|v| v.to_vec()),
        }
    }
}

/// Observe commits above State; only the executor's actual changes enter State.
#[derive(Debug)]
struct Observe<D: Database> {
    inner: State<D>,
    delta: Delta,
    error: Option<String>,
}

impl<D: Database> Observe<D> {
    fn new(db: D) -> Self {
        Self {
            inner: State::builder().with_database(db).with_bundle_update().build(),
            delta: Delta::default(),
            error: None,
        }
    }

    fn take(&mut self) -> eyre::Result<Delta> {
        eyre::ensure!(self.error.is_none(), "phase capture failed: {:?}", self.error);
        Ok(std::mem::take(&mut self.delta))
    }
}

impl<D: Database> reth_revm::Database for Observe<D> {
    type Error = <State<D> as reth_revm::Database>::Error;
    fn basic(&mut self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        self.inner.basic(address)
    }
    fn storage(&mut self, address: Address, slot: U256) -> Result<U256, Self::Error> {
        self.inner.storage(address, slot)
    }
    fn code_by_hash(&mut self, hash: B256) -> Result<Bytecode, Self::Error> {
        self.inner.code_by_hash(hash)
    }
    fn block_hash(&mut self, number: u64) -> Result<B256, Self::Error> {
        self.inner.block_hash(number)
    }
}

impl<D: Database> DatabaseCommit for Observe<D> {
    fn commit(&mut self, changes: EvmState) {
        let captured = changes
            .iter()
            .filter(|(_, a)| a.is_touched())
            .map(|(address, account)| {
                let deleted = account.is_selfdestructed() ||
                    (!account.is_created() && account.info.is_empty());
                let old = self.inner.basic(*address)?.unwrap_or_default();
                // Creation resets the old account, including code and storage.
                let base = if account.is_created() { AccountInfo::default() } else { old };
                let old = Account::from(&base);
                let new = Account::from(&account.info);
                Ok((
                    address.into_array(),
                    Change {
                        info: if deleted {
                            None
                        } else {
                            Some(InfoChange {
                                nonce: (new.nonce != old.nonce).then_some(new.nonce),
                                balance: (new.balance != old.balance)
                                    .then_some(new.balance.to_be_bytes()),
                                code_hash: (new.bytecode_hash != old.bytecode_hash)
                                    .then_some(new.bytecode_hash.map(|h| h.0)),
                            })
                        },
                        created: !deleted && account.is_created(),
                        storage: if deleted {
                            BTreeMap::new()
                        } else {
                            account
                                .storage
                                .iter()
                                .filter(|(_, s)| s.is_changed())
                                .map(|(k, s)| (k.to_be_bytes(), s.present_value.to_be_bytes()))
                                .collect()
                        },
                        code: if deleted || new.bytecode_hash == old.bytecode_hash {
                            None
                        } else {
                            account.info.code.as_ref().map(|c| c.original_bytes().to_vec())
                        },
                    },
                ))
            })
            .collect::<eyre::Result<BTreeMap<_, _>>>();
        match captured {
            Ok(delta) if !delta.is_empty() => self.delta.0.push(delta),
            Ok(_) => {}
            Err(error) => {
                self.error.get_or_insert_with(|| error.to_string());
            }
        }
        self.inner.commit(changes);
    }
}

impl Delta {
    /// Only independent-case derivation applies expected deltas. Full replay never calls this.
    fn apply<D: Database>(&self, db: &mut State<D>) -> eyre::Result<()> {
        for commit in &self.0 {
            let mut changes = EvmState::default();
            for (address, change) in commit {
                let address = Address::from(*address);
                let old = db.basic(address)?;
                let mut account = reth_revm::state::Account::default();
                account.mark_touch();
                if let Some(info) = &change.info {
                    account.info = if change.created {
                        AccountInfo::default()
                    } else {
                        old.unwrap_or_default()
                    };
                    if let Some(nonce) = info.nonce {
                        account.info.nonce = nonce;
                    }
                    if let Some(balance) = info.balance {
                        account.info.balance = U256::from_be_bytes(balance);
                    }
                    if let Some(hash) = info.code_hash {
                        account.info.code_hash =
                            hash.map(B256::from).unwrap_or(alloy_primitives::KECCAK256_EMPTY);
                        account.info.code = None;
                    }
                    if let Some(code) = &change.code {
                        account.info.code = Some(Bytecode::new_raw_checked(code.clone().into())?);
                    }
                    if change.created {
                        account.mark_created();
                    }
                    for (key, value) in &change.storage {
                        let key = U256::from_be_bytes(*key);
                        let old =
                            if change.created { U256::ZERO } else { db.storage(address, key)? };
                        account.storage.insert(
                            key,
                            EvmStorageSlot::new_changed(
                                old,
                                U256::from_be_bytes(*value),
                                Default::default(),
                            ),
                        );
                    }
                } else {
                    account.mark_selfdestruct();
                }
                changes.insert(address, account);
            }
            db.commit(changes);
        }
        Ok(())
    }
}

impl Transaction {
    pub fn gas_used(&self) -> u64 {
        self.outcome.charged_gas
    }

    /// Execute only this target after deriving its boundary, never executing prior transactions.
    pub fn replay<E: ConfigureEvm>(
        &self,
        config: &E,
        chunk: &Arc<Chunk>,
        prior: &[Self],
    ) -> eyre::Result<()> {
        let block = block::<E>(chunk)?;
        let mut db = Observe::new(View { chunk: chunk.clone(), at: chunk.start - 1 });
        let phases =
            chunk.phases.as_ref().ok_or_else(|| eyre::eyre!("missing phase expectations"))?;
        phases.pre.apply(&mut db.inner)?;
        eyre::ensure!(prior.len() == self.index as usize, "missing prior transaction deltas");
        for case in prior {
            case.delta.apply(&mut db.inner)?;
        }
        db.inner.bal_state.bal_index = BlockAccessIndex::new(self.index + 1);
        let evm = config.evm_with_env(&mut db, config.evm_env(block.header())?);
        let mut executor =
            config.block_executor_factory().create_executor(evm, config.context_for_block(&block)?);
        config.restore_transaction_boundary(&mut executor, &self.before)?;
        let tx = block
            .transactions_recovered()
            .nth(self.index as usize)
            .ok_or_else(|| eyre::eyre!("target index out of range"))?;
        eyre::ensure!(
            block.body().transactions()[self.index as usize].tx_hash().0 == self.hash,
            "target hash mismatch"
        );
        let result = executor.execute_transaction_without_commit(tx)?;
        eyre::ensure!(Outcome::capture(&result) == self.outcome, "transaction outcome mismatch");
        let receipt = alloy_rlp::decode_exact::<ReceiptTy<E::Primitives>>(&self.receipt)?;
        eyre::ensure!(result.result().result.logs() == receipt.logs(), "transaction logs mismatch");
        executor.commit_transaction(result);
        eyre::ensure!(
            config.transaction_boundary(&executor)?.as_ref() == self.after,
            "transaction boundary mismatch"
        );
        eyre::ensure!(
            alloy_rlp::encode(
                executor.receipts().last().ok_or_else(|| eyre::eyre!("missing receipt"))?
            ) == self.receipt,
            "transaction receipt mismatch"
        );
        eyre::ensure!(
            executor.evm_mut().db_mut().take()? == self.delta,
            "transaction delta mismatch"
        );
        Ok(())
    }
}

fn block<E: ConfigureEvm>(chunk: &Chunk) -> eyre::Result<RecoveredBlock<BlockTy<E::Primitives>>> {
    eyre::ensure!(chunk.blocks.len() == 1, "phase records require one block");
    let input = &chunk.blocks[0];
    let block = RecoveredBlock::new_unhashed(
        alloy_rlp::decode_exact::<BlockTy<E::Primitives>>(&input.rlp)?,
        input.senders.iter().copied().map(Address::from).collect(),
    );
    eyre::ensure!(
        block.hash().0 == input.hash &&
            block.number() == chunk.start &&
            block.parent_hash().0 == chunk.parent &&
            block.body().transactions().len() == input.senders.len(),
        "block context mismatch"
    );
    Ok(block)
}

/// One pre phase, one execution of each transaction, one post phase; all checks use that result.
pub(super) fn execute<E: ConfigureEvm, C: FullConsensus<E::Primitives> + ?Sized>(
    config: &E,
    consensus: &C,
    chunk: &Arc<Chunk>,
    first: u64,
    expected: Option<&[Transaction]>,
    cancellation: &CancellationToken,
) -> eyre::Result<(Vec<Transaction>, Phases)> {
    eyre::ensure!(!cancellation.is_cancelled(), "phase replay cancelled");
    let started = Instant::now();
    let block = block::<E>(chunk)?;
    let mut db = Observe::new(View { chunk: chunk.clone(), at: chunk.start - 1 });
    let has_bal = block.header().block_access_list_hash().is_some();
    if has_bal {
        db.inner.bal_state.bal_builder = Some(reth_revm::state::bal::Bal::new());
    }
    let evm = config.evm_with_env(&mut db, config.evm_env(block.header())?);
    let mut executor =
        config.block_executor_factory().create_executor(evm, config.context_for_block(&block)?);
    let setup = started.elapsed();
    let phase = Instant::now();
    executor.apply_pre_execution_changes()?;
    let pre = executor.evm_mut().db_mut().take()?;
    if expected.is_some() {
        eyre::ensure!(
            chunk.phases.as_ref().is_some_and(|p| p.pre == pre),
            "pre-execution delta mismatch"
        );
    }
    let pre_time = phase.elapsed();
    let phase = Instant::now();
    let mut cases = Vec::new();
    for (index, tx) in block.transactions_recovered().enumerate() {
        eyre::ensure!(!cancellation.is_cancelled(), "phase replay cancelled");
        if has_bal {
            executor.evm_mut().db_mut().inner.bump_bal_index();
        }
        let before = config.transaction_boundary(&executor)?.to_vec();
        let result = executor.execute_transaction_without_commit(tx)?;
        let outcome = Outcome::capture(&result);
        if let Some(expected) = expected {
            let case = expected
                .get(index)
                .ok_or_else(|| eyre::eyre!("missing transaction expectation"))?;
            let receipt = alloy_rlp::decode_exact::<ReceiptTy<E::Primitives>>(&case.receipt)?;
            eyre::ensure!(
                result.result().result.logs() == receipt.logs(),
                "transaction logs mismatch"
            );
        }
        executor.commit_transaction(result);
        let after = config.transaction_boundary(&executor)?.to_vec();
        let receipt = alloy_rlp::encode(
            executor.receipts().last().ok_or_else(|| eyre::eyre!("missing receipt"))?,
        );
        let delta = executor.evm_mut().db_mut().take()?;
        let case = Transaction {
            ordinal: first + index as u64,
            index: index as u64,
            hash: block.body().transactions()[index].tx_hash().0,
            before,
            after,
            outcome,
            receipt,
            delta,
        };
        if let Some(expected) = expected {
            eyre::ensure!(
                expected.get(index) == Some(&case),
                "transaction delta/outcome/context mismatch at {}",
                case.ordinal
            );
        }
        cases.push(case);
    }
    let tx_time = phase.elapsed();
    let phase = Instant::now();
    if has_bal {
        executor.evm_mut().db_mut().inner.bump_bal_index();
    }
    let (evm, result) = executor.finish()?;
    let db = evm.into_db();
    let post = db.take()?;
    let post_time = phase.elapsed();
    let phase = Instant::now();
    if let Some(expected) = expected {
        eyre::ensure!(expected.len() == cases.len(), "transaction count mismatch");
        eyre::ensure!(
            chunk.phases.as_ref().is_some_and(|p| p.post == post),
            "post-execution delta mismatch"
        );
    }
    let bal_hash = db
        .inner
        .take_built_alloy_bal()
        .map(|b| Bal::from(b).compute_hash_with_buf(&mut Vec::new()));
    consensus.validate_block_post_execution(&block, &result, None, bal_hash)?;
    if expected.is_none() {
        // Capture binds the generated deltas to independently read canonical poststate.
        // These poststate versions are discarded before publication, not persisted as
        // a second expected-state representation alongside the phase deltas.
        for (address, versions) in &chunk.accounts {
            if let Some((at, info)) = versions.last() &&
                *at == chunk.start
            {
                let actual = db.inner.basic(Address::from(*address))?;
                eyre::ensure!(
                    actual.as_ref().map(|a| Info::capture(&Account::from(a))).transpose()? == *info,
                    "captured account delta disagrees with canonical poststate"
                );
            }
        }
        for ((address, slot), versions) in &chunk.storage {
            if let Some((at, value)) = versions.last() &&
                *at == chunk.start
            {
                eyre::ensure!(
                    db.inner
                        .storage(Address::from(*address), U256::from_be_bytes(*slot))?
                        .to_be_bytes::<32>() ==
                        *value,
                    "captured storage delta disagrees with canonical poststate"
                );
            }
        }
    }
    db.inner.merge_transitions(BundleRetention::Reverts);
    super::verify_bundle_against_changesets(chunk.as_ref(), &db.inner.take_bundle(), chunk.start)?;
    tracing::debug!(target: "reth::corpus::phases", block = chunk.start, transactions = cases.len(), setup_us = setup.as_micros() as u64, pre_us = pre_time.as_micros() as u64, tx_us = tx_time.as_micros() as u64, post_us = post_time.as_micros() as u64, checks_us = phase.elapsed().as_micros() as u64, "Single-pass phase intervals (overlap across workers)");
    Ok((cases, Phases { pre, post }))
}
