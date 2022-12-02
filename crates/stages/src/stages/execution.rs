use crate::{
    db::StageDB, DatabaseIntegrityError, ExecInput, ExecOutput, Stage, StageError, StageId,
    UnwindInput, UnwindOutput,
};
use reth_executor::{
    config::SpecUpgrades,
    executor::AccountChangeSet,
    revm_wrap::{State, SubState},
    Config,
};
use reth_interfaces::{
    db::{models::BlockNumHash, tables, Database, DbCursorRO, DbTx, DbTxMut},
    provider::db::StateProviderImplRefLatest,
};
use reth_primitives::{StorageEntry, TransactionSignedEcRecovered, H256};
use std::{fmt::Debug, ops::DerefMut};

const EXECUTION: StageId = StageId("Execution");

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
pub struct ExecutionStage;

/// Specify batch sizes of block in execution
/// TODO make this as config
const BATCH_SIZE: u64 = 1000;

#[async_trait::async_trait]
impl<DB: Database> Stage<DB> for ExecutionStage {
    /// Return the id of the stage
    fn id(&self) -> StageId {
        EXECUTION
    }

    /// Execute the stage
    async fn execute(
        &mut self,
        db: &mut StageDB<'_, DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        let db_tx = db.deref_mut();
        let last_block = input.stage_progress.unwrap_or_default();
        let start_block = last_block + 1;

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

        // get canonical blocks (num,hash)
        let canonical_batch = canonicals
            .walk(start_block)?
            .take(BATCH_SIZE as usize)
            .map(|i| i.map(BlockNumHash))
            .collect::<Result<Vec<_>, _>>()?;

        // no more canonical blocks, we are done with execution.
        if canonical_batch.is_empty() {
            return Ok(ExecOutput { done: true, reached_tip: true, stage_progress: last_block })
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

            // execution and store output to results
            results.extend(
                reth_executor::executor::execute_and_verify_receipt(
                    header,
                    &recovered_transactions,
                    &config,
                    &mut state_provider,
                )
                .map_err(|error| StageError::ExecutionError { block: header.number, error })?,
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
                    // if it is none it means it is just storage change.
                    if account.0.is_some() {
                        db_tx.delete::<tables::PlainAccountState>(address, None)?;
                    }
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
                // make different types of bytecode. Checked and maybe even analyzed (needs to be
                // packed). Currently save only raw bytes.
                db_tx
                    .put::<tables::Bytecodes>(hash, bytecode.bytes()[..bytecode.len()].to_vec())?;
            }
        }

        // TODO insert Account/Storage ChangeSet

        let last_block = start_block + canonical_batch.len() as u64;
        let is_done = canonical_batch.len() < BATCH_SIZE as usize;
        Ok(ExecOutput { done: is_done, reached_tip: true, stage_progress: last_block })
    }

    /// Unwind the stage.
    async fn unwind(
        &mut self,
        _db: &mut StageDB<'_, DB>,
        _input: UnwindInput,
    ) -> Result<UnwindOutput, Box<dyn std::error::Error + Send + Sync>> {
        panic!("For unwindng we need Account/Storage ChangeSet");
        //Ok(UnwindOutput { stage_progress: input.unwind_to })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_db::{
        kv::{test_utils::create_test_db, EnvKind},
        mdbx::WriteMap,
    };
    use reth_interfaces::provider::insert_canonical_block;
    use reth_primitives::{hex_literal::hex, keccak256, Account, BlockLocked, H160, U256};
    use reth_rlp::Decodable;

    #[tokio::test]
    async fn sanity_execution_of_block() {
        let state_db = create_test_db::<WriteMap>(EnvKind::RW);
        let mut db = StageDB::new(state_db.as_ref()).unwrap();
        let input = ExecInput {
            previous_stage: None,
            /// The progress of this stage the last time it was executed.
            stage_progress: None,
        };
        let mut genesis_rlp = hex!("f901faf901f5a00000000000000000000000000000000000000000000000000000000000000000a01dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347942adc25665018aa1fe0e6bc666dac8fc2697ff9baa045571b40ae66ca7480791bbb2887286e4e4c4b1b298b191c889d6959023a32eda056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421b901000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000083020000808502540be400808000a00000000000000000000000000000000000000000000000000000000000000000880000000000000000c0c0").as_slice();
        let genesis = BlockLocked::decode(&mut genesis_rlp).unwrap();
        let mut block_rlp = hex!("f90262f901f9a075c371ba45999d87f4542326910a11af515897aebce5265d3f6acd1f1161f82fa01dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347942adc25665018aa1fe0e6bc666dac8fc2697ff9baa098f2dcd87c8ae4083e7017a05456c14eea4b1db2032126e27b3b1563d57d7cc0a08151d548273f6683169524b66ca9fe338b9ce42bc3540046c828fd939ae23bcba03f4e5c2ec5b2170b711d97ee755c160457bb58d8daa338e835ec02ae6860bbabb901000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000083020000018502540be40082a8798203e800a00000000000000000000000000000000000000000000000000000000000000000880000000000000000f863f861800a8405f5e10094100000000000000000000000000000000000000080801ba07e09e26678ed4fac08a249ebe8ed680bf9051a5e14ad223e4b2b9d26e0208f37a05f6e3f188e3e6eab7d7d3b6568f5eac7d687b08d307d3154ccd8c87b4630509bc0").as_slice();
        let block = BlockLocked::decode(&mut block_rlp).unwrap();
        insert_canonical_block(db.deref_mut(), &genesis).unwrap();
        insert_canonical_block(db.deref_mut(), &block).unwrap();
        db.commit().unwrap();

        // insert pre state
        let tx = db.deref_mut();
        let acc1 = H160(hex!("1000000000000000000000000000000000000000"));
        let acc2 = H160(hex!("a94f5374fce5edbc8e2a8697c15331677e6ebf0b"));
        let code = hex!("5a465a905090036002900360015500");
        let balance = U256::from(0x3635c9adc5dea00000u128);
        let code_hash = keccak256(code);
        tx.put::<tables::PlainAccountState>(
            acc1,
            Account { nonce: 0, balance: 0.into(), bytecode_hash: Some(code_hash) },
        )
        .unwrap();
        tx.put::<tables::PlainAccountState>(
            acc2,
            Account { nonce: 0, balance, bytecode_hash: None },
        )
        .unwrap();
        tx.put::<tables::Bytecodes>(code_hash, code.to_vec()).unwrap();
        db.commit().unwrap();

        // execute
        let _o = ExecutionStage.execute(&mut db, input).await.unwrap();
        db.commit().unwrap();
        let tx = db.deref_mut();
        // check post state
        let account1 = H160(hex!("1000000000000000000000000000000000000000"));
        let account1_info =
            Account { balance: 0x00.into(), nonce: 0x00, bytecode_hash: Some(code_hash) };
        let account2 = H160(hex!("2adc25665018aa1fe0e6bc666dac8fc2697ff9ba"));
        let account2_info = Account {
            // TODO remove 2eth block reward
            balance: (0x1bc16d674ece94bau128 - 0x1bc16d674ec80000u128).into(),
            nonce: 0x00,
            bytecode_hash: Some(H256(hex!(
                "c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470"
            ))),
        };
        let account3 = H160(hex!("a94f5374fce5edbc8e2a8697c15331677e6ebf0b"));
        let account3_info = Account {
            balance: 0x3635c9adc5de996b46u128.into(),
            nonce: 0x01,
            bytecode_hash: Some(H256(hex!(
                "c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470"
            ))),
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
        // Get on dupsort would return only first value. This is good enought for this test.
        assert_eq!(
            tx.get::<tables::PlainStorageState>(account1),
            Ok(Some(StorageEntry { key: H256::from_low_u64_be(1), value: 2.into() })),
            "Post changed of a account"
        );
    }
}
