use crate::{
    db::StageDB, DatabaseIntegrityError, ExecInput, ExecOutput, Stage, StageError, StageId,
    UnwindInput, UnwindOutput,
};
use reth_db::{
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO},
    database::Database,
    models::{BlockNumHash, TxNumberAddress},
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_executor::{
    config::SpecUpgrades,
    executor::AccountChangeSet,
    revm_wrap::{State, SubState},
    Config,
};
use reth_primitives::{Address, StorageEntry, TransactionSignedEcRecovered, H256, U256};
use reth_provider::StateProviderImplRefLatest;
use std::{fmt::Debug, ops::DerefMut};

const EXECUTION: StageId = StageId("Execution");

/// The execution stage executes all transactions and
/// update history indexes.
///
/// Input tables:
/// [tables::CanonicalHeaders] get next block to execute.
/// [tables::Headers] get for revm environment variables.
/// [tables::CumulativeTxCount] to get tx number
/// [tables::Transactions] to execute
///
/// For state access [StateProvider] provides us latest state and history state
/// For latest most recent state [StateProvider] would need (Used for execution Stage):
/// [tables::PlainAccountState]
/// [tables::Bytecodes]
/// [tables::PlainStorageState]
///
/// Tables updated after state finishes execution:
/// [tables::PlainAccountState]
/// [tables::PlainStorageState]
/// [tables::Bytecodes]
/// [tables::AccountChangeSet]
/// [tables::StorageChangeSet]
///
/// For unwinds we are accessing:
/// [tables::CumulativeTxCount] get tx index to know what needs to be unwinded
/// [tables::AccountHistory] to remove change set and apply old values to
/// [tables::PlainAccountState] [tables::StorageHistory] to remove change set and apply old values
/// to [tables::PlainStorageState]
#[derive(Debug)]
pub struct ExecutionStage {
    config: Config,
}

impl Default for ExecutionStage {
    fn default() -> Self {
        Self { config: Config { chain_id: 1.into(), spec_upgrades: SpecUpgrades::new_ethereum() } }
    }
}

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
        // none and zero are same as for genesis block (zeroed block) we are making assumption to
        // not have transaction.
        let last_block = input.stage_progress.unwrap_or_default();
        let start_block = last_block + 1;

        // Get next canonical block hashes to execute.
        let mut canonicals = db_tx.cursor::<tables::CanonicalHeaders>()?;
        // Get header with canonical hashes.
        let mut headers = db_tx.cursor::<tables::Headers>()?;
        // Get bodies (to get tx index) with canonical hashes.
        let mut cumulative_tx_count = db_tx.cursor::<tables::CumulativeTxCount>()?;
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
        let headers_batch = canonical_batch
            .iter()
            .map(|ch_index| {
                // TODO see if walker next has better performance then seek_exact calls.
                headers.seek_exact(*ch_index).map_err(StageError::Database).and_then(|res| {
                    res.ok_or_else(|| {
                        DatabaseIntegrityError::Header {
                            number: ch_index.number(),
                            hash: ch_index.hash(),
                        }
                        .into()
                    })
                    .map(|(_, header)| header)
                })
            })
            .collect::<Result<Vec<_>, _>>()?;

        // get last tx count so that we can know amount of transaction in the block.
        let mut last_tx_index = if last_block == 0 {
            0u64
        } else {
            // headers_batch is not empty,
            let parent_hash = headers_batch[0].parent_hash;

            let (_, tx_cnt) = cumulative_tx_count
                .seek_exact(BlockNumHash((last_block, parent_hash)))?
                .ok_or(DatabaseIntegrityError::CumulativeTxCount {
                    number: last_block,
                    hash: parent_hash,
                })?;
            tx_cnt
        };

        let tx_index_ranges = canonical_batch
            .iter()
            .map(|ch_index| {
                // TODO see if walker next has better performance then seek_exact calls.
                cumulative_tx_count
                    .seek_exact(*ch_index)
                    .map_err(StageError::Database)
                    .and_then(|res| {
                        res.ok_or_else(|| {
                            DatabaseIntegrityError::CumulativeTxCount {
                                number: ch_index.number(),
                                hash: ch_index.hash(),
                            }
                            .into()
                        })
                    })
                    .map(|(_, cumulative_tx_count)| {
                        let ret = if self.config.spec_upgrades.has_block_reward(ch_index.number()) {
                            // if there is block reward, cumulative tx count needs to remove block
                            // reward index. It is okay to subtract it, as
                            // block reward index is calculated in the block stage.
                            (last_tx_index, cumulative_tx_count - 1, Some(cumulative_tx_count - 1))
                        } else {
                            // if there is no block reward we just need to use tx_count
                            (last_tx_index, cumulative_tx_count, None)
                        };
                        last_tx_index = cumulative_tx_count;
                        ret
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;

        // Fetch transactions, execute them and generate results
        let mut block_change_patches = Vec::with_capacity(canonical_batch.len());
        for (header, (start_tx_index, end_tx_index, block_reward_index)) in
            headers_batch.iter().zip(tx_index_ranges.iter())
        {
            let body_tx_cnt = end_tx_index - start_tx_index;
            // iterate over all transactions
            let mut tx_walker = tx.walk(*start_tx_index)?;
            let mut transactions = Vec::with_capacity(body_tx_cnt as usize);
            // get next N transactions.
            for index in *start_tx_index..*end_tx_index {
                let (tx_index, tx) =
                    tx_walker.next().ok_or(DatabaseIntegrityError::EndOfTransactionTable)??;
                if tx_index != index {
                    return Err(DatabaseIntegrityError::TransactionsGap { missing: tx_index }.into())
                }
                transactions.push(tx);
            }

            // take signers
            let mut tx_sender_walker = tx_sender.walk(*start_tx_index)?;
            let mut signers = Vec::with_capacity(body_tx_cnt as usize);
            for index in *start_tx_index..*end_tx_index {
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

            let mut state_provider =
                SubState::new(State::new(StateProviderImplRefLatest::new(db_tx)));

            // executiong and store output to results
            block_change_patches.push((
                reth_executor::executor::execute_and_verify_receipt(
                    header,
                    &recovered_transactions,
                    &self.config,
                    &mut state_provider,
                )
                .map_err(|error| StageError::ExecutionError { block: header.number, error })?,
                start_tx_index,
                block_reward_index,
            ));
        }

        // apply changes to plain database.
        for (results, start_tx_index, block_reward_index) in block_change_patches.into_iter() {
            // insert state change set
            for (index, result) in results.changeset.into_iter().enumerate() {
                let tx_index = start_tx_index + index as u64;
                for (address, AccountChangeSet { account, wipe_storage, storage }) in
                    result.state_diff.into_iter()
                {
                    // apply account change to db. Updates AccountChangeSet and PlainAccountState
                    // tables.
                    account.apply_to_db(address, tx_index, db_tx)?;

                    // wipe storage
                    if wipe_storage {
                        // TODO insert all changes to StorageChangeSet
                        db_tx.delete::<tables::PlainStorageState>(address, None)?;
                    }
                    // insert storage changeset
                    let storage_id = TxNumberAddress((tx_index, address));
                    for (key, (old_value, new_value)) in storage {
                        let mut hkey = H256::zero();
                        key.to_big_endian(&mut hkey.0);

                        // insert into StorageChangeSet
                        db_tx.put::<tables::StorageChangeSet>(
                            storage_id.clone(),
                            StorageEntry { key: hkey, value: old_value },
                        )?;

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
                    // make different types of bytecode. Checked and maybe even analyzed (needs to
                    // be packed). Currently save only raw bytes.
                    db_tx.put::<tables::Bytecodes>(
                        hash,
                        bytecode.bytes()[..bytecode.len()].to_vec(),
                    )?;

                    // NOTE: bytecode bytes are not inserted in change set and it stand in saparate
                    // table
                }
            }

            // If there is block reward we will add account changeset to db
            // TODO add apply_block_reward_changeset to db tx fn which maybe takes an option.
            if let Some(block_reward_changeset) = results.block_reward {
                // we are sure that block reward index is present.
                let block_reward_index = block_reward_index.unwrap();
                for (address, changeset) in block_reward_changeset.into_iter() {
                    changeset.apply_to_db(address, block_reward_index, db_tx)?;
                }
            }
        }

        let last_block = last_block + canonical_batch.len() as u64;
        let is_done = canonical_batch.len() < BATCH_SIZE as usize;
        Ok(ExecOutput { done: is_done, reached_tip: true, stage_progress: last_block })
    }

    /// Unwind the stage.
    async fn unwind(
        &mut self,
        db: &mut StageDB<'_, DB>,
        input: UnwindInput,
    ) -> Result<UnwindOutput, Box<dyn std::error::Error + Send + Sync>> {
        let unwind_from = input.stage_progress;
        let unwind_to = input.unwind_to;
        let _bad_block = input.bad_block;

        // get block body tx indexes
        let db_tx = db.deref_mut();

        // Get transaction of the block that we are executing.
        let mut account_changeset = db_tx.cursor_dup_mut::<tables::AccountChangeSet>()?;
        // Skip sender recovery and load signer from database.
        let mut storage_changeset = db_tx.cursor_dup_mut::<tables::StorageChangeSet>()?;

        // get from tx_number
        let unwind_from_hash = db_tx
            .get::<tables::CanonicalHeaders>(unwind_from)?
            .ok_or(DatabaseIntegrityError::CanonicalHeader { number: unwind_from })?;

        let from_tx_number = db_tx
            .get::<tables::CumulativeTxCount>(BlockNumHash((unwind_from, unwind_from_hash)))?
            .ok_or(DatabaseIntegrityError::CumulativeTxCount {
                number: unwind_from,
                hash: unwind_from_hash,
            })?;

        // get to tx_number
        let to_tx_number = if unwind_to == 0 {
            0
        } else {
            let parent_number = unwind_to - 1;
            let parent_hash = db_tx
                .get::<tables::CanonicalHeaders>(parent_number)?
                .ok_or(DatabaseIntegrityError::CanonicalHeader { number: parent_number })?;

            db_tx
                .get::<tables::CumulativeTxCount>(BlockNumHash((parent_number, parent_hash)))?
                .ok_or(DatabaseIntegrityError::CumulativeTxCount {
                    number: parent_number,
                    hash: parent_hash,
                })?
        };

        if to_tx_number > from_tx_number {
            panic!("Parents CumulativeTxCount {to_tx_number} is higer then present block #{unwind_to} TxCount {from_tx_number}")
        }
        let num_of_tx = (from_tx_number - to_tx_number) as usize;

        // if there is no transaction ids, this means blocks were empty and block reward change set
        // is not present.
        if num_of_tx == 0 {
            return Ok(UnwindOutput { stage_progress: unwind_to })
        }

        // get all batches for account change
        // Check if walk and walk_dup would do the same thing
        let account_changeset_batch = account_changeset
            .walk_dup(to_tx_number, Address::zero())?
            .take_while(|item| item.as_ref().map_or(false, |(num, _)| *num <= from_tx_number))
            .collect::<Result<Vec<_>, _>>()?;

        // revert all changes to PlainState
        for (_, changeset) in account_changeset_batch.into_iter().rev() {
            // TODO refactor in db fn called tx.aplly_account_changeset
            if let Some(account_info) = changeset.info {
                db_tx.put::<tables::PlainAccountState>(changeset.address, account_info)?;
            } else {
                db_tx.delete::<tables::PlainAccountState>(changeset.address, None)?;
            }
        }

        // get all batches for storage change
        let storage_chageset_batch = storage_changeset
            .walk_dup(TxNumberAddress((to_tx_number, Address::zero())), H256::zero())?
            .take_while(|item| {
                item.as_ref().map_or(false, |(TxNumberAddress((num, _)), _)| *num <= from_tx_number)
            })
            .collect::<Result<Vec<_>, _>>()?;

        // revert all changes to PlainStorage
        for (TxNumberAddress((_, address)), storage) in storage_chageset_batch.into_iter().rev() {
            db_tx.put::<tables::PlainStorageState>(address, storage.clone())?;
            if storage.value == U256::zero() {
                // delete value that is zero
                db_tx.delete::<tables::PlainStorageState>(address, Some(storage))?;
            }
        }

        // Discard unwinded changesets
        let mut entry = account_changeset.last()?;
        while let Some((tx_number, _)) = entry {
            if tx_number < to_tx_number {
                break
            }
            account_changeset.delete_current()?;
            entry = account_changeset.prev()?;
        }

        let mut entry = storage_changeset.last()?;
        while let Some((TxNumberAddress((tx_number, _)), _)) = entry {
            if tx_number < to_tx_number {
                break
            }
            storage_changeset.delete_current()?;
            entry = storage_changeset.prev()?;
        }

        Ok(UnwindOutput { stage_progress: input.unwind_to })
    }
}

#[cfg(test)]
mod tests {
    use std::ops::Deref;

    use super::*;
    use reth_db::mdbx::{test_utils::create_test_db, EnvKind, WriteMap};
    use reth_primitives::{hex_literal::hex, keccak256, Account, BlockLocked, H160, U256};
    use reth_provider::insert_canonical_block;
    use reth_rlp::Decodable;

    #[tokio::test]
    async fn sanity_execution_of_block() {
        // TODO cleanup the setup after https://github.com/paradigmxyz/reth/issues/332
        // is merged as it has similar framework
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
        insert_canonical_block(db.deref_mut(), &genesis, true).unwrap();
        insert_canonical_block(db.deref_mut(), &block, true).unwrap();
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
        let mut execution_stage = ExecutionStage::default();
        execution_stage.config.spec_upgrades = SpecUpgrades::new_berlin_activated();
        let output = execution_stage.execute(&mut db, input).await.unwrap();
        db.commit().unwrap();
        assert_eq!(output, ExecOutput { stage_progress: 1, done: true, reached_tip: true });
        let tx = db.deref_mut();
        // check post state
        let account1 = H160(hex!("1000000000000000000000000000000000000000"));
        let account1_info =
            Account { balance: 0x00.into(), nonce: 0x00, bytecode_hash: Some(code_hash) };
        let account2 = H160(hex!("2adc25665018aa1fe0e6bc666dac8fc2697ff9ba"));
        let account2_info =
            Account { balance: (0x1bc16d674ece94bau128).into(), nonce: 0x00, bytecode_hash: None };
        let account3 = H160(hex!("a94f5374fce5edbc8e2a8697c15331677e6ebf0b"));
        let account3_info =
            Account { balance: 0x3635c9adc5de996b46u128.into(), nonce: 0x01, bytecode_hash: None };

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

    #[tokio::test]
    async fn sanity_execute_unwind() {
        // TODO cleanup the setup after https://github.com/paradigmxyz/reth/issues/332
        // is merged as it has similar framework

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
        insert_canonical_block(db.deref_mut(), &genesis, true).unwrap();
        insert_canonical_block(db.deref_mut(), &block, true).unwrap();
        db.commit().unwrap();

        // variables
        let code = hex!("5a465a905090036002900360015500");
        let balance = U256::from(0x3635c9adc5dea00000u128);
        let code_hash = keccak256(code);
        // pre state
        let tx = db.deref_mut();
        let acc1 = H160(hex!("1000000000000000000000000000000000000000"));
        let acc1_info = Account { nonce: 0, balance: 0.into(), bytecode_hash: Some(code_hash) };
        let acc2 = H160(hex!("a94f5374fce5edbc8e2a8697c15331677e6ebf0b"));
        let acc2_info = Account { nonce: 0, balance, bytecode_hash: None };

        tx.put::<tables::PlainAccountState>(acc1, acc1_info).unwrap();
        tx.put::<tables::PlainAccountState>(acc2, acc2_info).unwrap();
        tx.put::<tables::Bytecodes>(code_hash, code.to_vec()).unwrap();
        db.commit().unwrap();

        // execute

        let mut execution_stage = ExecutionStage::default();
        execution_stage.config.spec_upgrades = SpecUpgrades::new_berlin_activated();
        let _ = execution_stage.execute(&mut db, input).await.unwrap();
        db.commit().unwrap();

        let o = ExecutionStage::default()
            .unwind(&mut db, UnwindInput { stage_progress: 1, unwind_to: 0, bad_block: None })
            .await
            .unwrap();

        assert_eq!(o, UnwindOutput { stage_progress: 0 });

        // assert unwind stage
        let tx = db.deref();
        assert_eq!(
            tx.get::<tables::PlainAccountState>(acc1),
            Ok(Some(acc1_info)),
            "Pre changed of a account"
        );
        assert_eq!(
            tx.get::<tables::PlainAccountState>(acc2),
            Ok(Some(acc2_info)),
            "Post changed of a account"
        );

        let miner_acc = H160(hex!("2adc25665018aa1fe0e6bc666dac8fc2697ff9ba"));
        assert_eq!(
            tx.get::<tables::PlainAccountState>(miner_acc),
            Ok(None),
            "Third account should be unwinded"
        );
    }
}
