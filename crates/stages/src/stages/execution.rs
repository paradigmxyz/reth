use crate::{
    DatabaseIntegrityError, ExecInput, ExecOutput, Stage, StageError, StageId, UnwindInput,
    UnwindOutput,
};
use reth_executor::{
    config::SpecUpgrades,
    executor::AccountChangeSet,
    revm_wrap::{State, SubState},
    Config,
};
use reth_interfaces::{
    db::{models::BlockNumHash, tables, DBContainer, Database, DbCursorRO, DbTx, DbTxMut},
    provider::db::StateProviderImplRefLatest,
};
use reth_primitives::{StorageEntry, TransactionSignedEcRecovered, H256};
use std::fmt::Debug;

const TX_INDEX: StageId = StageId("Execution");

/// The execution stage executes all transactions and
/// update history indexes.
///
/// Input:
/// [tables::CanonicalHeaders] get next block to execute.
/// [tables::Headers] get for env
/// [tables::BlockBodies] to get tx number
/// [tables::Transactions] to execute
///
/// [StateProvider] needed for execution on most recent state:
/// [tables::PlainAccountState]
/// [tables::Bytecodes]
/// [tables::PlainStorageState]
///
/// [StateProvider] needed for execution on history state (Not needed for stage):
/// [tables::AccountHistory]
/// [tables::Bytecodes]
/// [tables::StorageHistory]
/// [tables::AccountChangeSet]
/// [tables::StorageChangeSet]
///
/// Output:
/// [tables::PlainAccountState]
/// [tables::PlainStorageState]
/// [tables::Bytecodes]
/// [tables::AccountChangeSet]
/// [tables::StorageChangeSet]
/// [tables::AccountHistory] and [tables::StorageHistory] indexes can be updated later in next
/// stage?
///
/// For unwinds:
/// [tables::BlockBodies] get tx index to know what needs to be unwinded
/// [tables::AccountHistory] remove change set and apply old values to [tables::PlainAccountState]
/// [tables::StorageHistory] remove change set and apply old values to [tables::PlainStorageState]
#[derive(Debug)]
pub struct Execution;

/// SPecify batch sizes of block in execution
/// TODO make this as config
const BATCH_SIZE: u64 = 1000;

#[async_trait::async_trait]
impl<DB: Database> Stage<DB> for Execution {
    /// Return the id of the stage
    fn id(&self) -> StageId {
        TX_INDEX
    }

    /// Execute the stage
    async fn execute(
        &mut self,
        db: &mut DBContainer<'_, DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        let db_tx = db.get_mut();
        let last_block = input.stage_progress.unwrap_or_default();
        let start_block = last_block + 1;
        let end_block = start_block + BATCH_SIZE;

        // Get next canonical block hashes to execute.
        let mut canonicals = db_tx.cursor::<tables::CanonicalHeaders>()?;
        // Get header with canonical hashes.
        let mut headers = db_tx.cursor::<tables::Headers>()?;
        // Get bodies (to get tx index) with canonical hashes.
        let mut bodies = db_tx.cursor::<tables::BlockBodies>()?;
        // Get transaction of the block that we are executing.
        let mut tx = db_tx.cursor::<tables::Transactions>()?;
        // Skip sender recovery and load signer from database.
        let mut tx_sender = db_tx.cursor::<tables::TxSenders>()?;

        // get canonical block (num,hash)
        let mut canonical_batch = Vec::new();
        let mut canonicals_walker = canonicals.walk(start_block)?;
        for _ in start_block..end_block {
            if let Some(ch) = canonicals_walker.next() {
                canonical_batch.push(BlockNumHash(ch?))
            } else {
                break
            }
        }

        // get headers from canonical numbers
        let mut headers_batch = Vec::with_capacity(canonical_batch.len());
        for ch_index in canonical_batch.iter() {
            // TODO see if walker next has better performance then seek_exact calls.
            let (_, header) =
                headers.seek_exact(*ch_index)?.ok_or(DatabaseIntegrityError::Header {
                    number: ch_index.number(),
                    hash: ch_index.hash(),
                })?;
            headers_batch.push(header);
        }

        // get block
        let mut body_batch = Vec::with_capacity(canonical_batch.len());
        for ch_index in canonical_batch.iter() {
            // TODO see if walker next has better performance then seek_exact calls.
            let (_, block) = bodies
                .seek_exact(*ch_index)?
                .ok_or(DatabaseIntegrityError::BlockBody { number: ch_index.number() })?;
            body_batch.push(block);
        }

        // Fetch transactions, execute them and generate results
        let mut results = Vec::new();
        for (header, body) in headers_batch.iter().zip(body_batch.iter()) {
            let start_tx_index = body.base_tx_id;
            let end_tx_index = body.tx_amount + start_tx_index;
            // iterate over all transactions
            let mut tx_walker = tx.walk(start_tx_index)?;
            let mut transactions = Vec::with_capacity(body.tx_amount as usize);
            // get next N transactions.
            for index in start_tx_index..end_tx_index {
                let (tx_index, tx) =
                    tx_walker.next().ok_or(DatabaseIntegrityError::EndOfTransactionTable)??;
                if tx_index != index {
                    return Err(DatabaseIntegrityError::TransactionsGap { missing: tx_index }.into())
                }
                transactions.push(tx);
            }

            // take signers
            let mut tx_sender_walker = tx_sender.walk(start_tx_index)?;
            let mut signers = Vec::with_capacity(body.tx_amount as usize);
            for index in start_tx_index..end_tx_index {
                let (tx_index, tx) = tx_sender_walker
                    .next()
                    .ok_or(DatabaseIntegrityError::EndOfTransactionSenderTable)??;
                if tx_index != index {
                    return Err(
                        DatabaseIntegrityError::TransactionsSignerGap { missing: tx_index }.into()
                    )
                }
                signers.push(tx);
            }
            // create ecRecovered transaction by matching tx and its signer
            let recovered_transactions: Vec<_> = transactions
                .into_iter()
                .zip(signers.into_iter())
                .map(|(tx, signer)| {
                    TransactionSignedEcRecovered::from_signed_transactions_and_signer(tx, signer)
                })
                .collect();

            // for now use default eth config
            let config = Config { chain_id: 1.into(), spec_upgrades: SpecUpgrades::new_ethereum() };

            let mut state_provider =
                SubState::new(State::new(StateProviderImplRefLatest::new(db_tx)));

            // executiong and store output to results
            results.extend(
                reth_executor::executor::execute_and_verify_receipt(
                    header,
                    &recovered_transactions,
                    &config,
                    &mut state_provider,
                )
                .map_err(|_| DatabaseIntegrityError::EndOfTransactionTable)?,
            );
        }

        // apply changes to plain database.
        for result in results.into_iter() {
            // insert account diff
            for (address, AccountChangeSet { account, wipe_storage, storage }) in
                result.state_diff.into_iter()
            {
                // insert account
                if let Some(account) = account.1 {
                    db_tx.put::<tables::PlainAccountState>(address, account)?;
                } else {
                    db_tx.delete::<tables::PlainAccountState>(address, None)?;
                }
                // wipe storage
                if wipe_storage {
                    db_tx.delete::<tables::PlainStorageState>(address, None)?;
                }
                // insert storage diff
                for (key, (old_value, new_value)) in storage {
                    let mut hkey = H256::zero();
                    key.to_big_endian(&mut hkey.0);
                    if new_value.is_zero() {
                        db_tx.delete::<tables::PlainStorageState>(
                            address,
                            Some(StorageEntry { key: hkey, value: old_value }),
                        )?;
                    } else {
                        db_tx.put::<tables::PlainStorageState>(
                            address,
                            StorageEntry { key: hkey, value: new_value },
                        )?;
                    }
                }
            }
            // insert bytecode
            for (hash, bytecode) in result.new_bytecodes.into_iter() {
                // make different types of bytecode. Checked and maybe even analyzed (needs to be packed).
                // Currently save only raw bytes.
                db_tx
                    .put::<tables::Bytecodes>(hash, bytecode.bytes()[..bytecode.len()].to_vec())?;
            }
        }

        // TODO insert Account/Storage ChangeSet

        let last_block = start_block + canonical_batch.len() as u64;
        let is_done = canonical_batch.len() != BATCH_SIZE as usize;
        // TODO not sure what exactly to set `reached_tip` to.
        Ok(ExecOutput { done: is_done, reached_tip: true, stage_progress: last_block })
    }

    /// Unwind the stage.
    async fn unwind(
        &mut self,
        _db: &mut DBContainer<'_, DB>,
        _input: UnwindInput,
    ) -> Result<UnwindOutput, Box<dyn std::error::Error + Send + Sync>> {
        panic!("For unwindng we need Account/Storage ChangeSet");
        //Ok(UnwindOutput { stage_progress: input.unwind_to })
    }
}

#[cfg(test)]
mod tests {
    // TODO sanity test
}
