use crate::{
    revm_wrap::{self, SubState},
    Config,
};
use hashbrown::hash_map::Entry;
use reth_interfaces::{executor::Error, provider::StateProvider};
use reth_primitives::{
    bloom::logs_bloom, Account, Address, Bloom, Log, Receipt, SealedHeader,
    TransactionSignedEcRecovered, H256, U256,
};
use revm::{
    db::AccountState, Account as RevmAccount, AccountInfo, AnalysisKind, Bytecode, ExecutionResult,
    EVM,
};
use std::collections::BTreeMap;

/// Main block executor
pub struct Executor {
    /// Configuration, Spec and optional flags.
    pub config: Config,
}

/// Diff change set that is neede for creating history index and updating current world state.
#[derive(Debug, Clone)]
pub struct AccountChangeSet {
    /// Old and New account.
    /// If account is deleted (selfdestructed) we would have (Some,None).
    /// If account is newly created we would have (None,Some).
    /// If it is only a storage change we would have (None,None)
    /// If account is changed (balance,nonce) we would have (Some,Some)
    pub account: (Option<Account>, Option<Account>),
    /// Storage containing key -> (OldValue,NewValue). in case that old value is not existing
    /// we can expect to have U256::zero(), same with new value.
    pub storage: BTreeMap<U256, (U256, U256)>,
    /// Just to make sure that we are taking selfdestruct cleaning we have this field that wipes
    /// storage. There are instances where storage is changed but account is not touched, so we
    /// can't take into account that if new account is None that it is selfdestruct.
    pub wipe_storage: bool,
}

/// Commit chgange to database and return change diff that is used to update state and create
/// history index
///
/// ChangeDiff consists of:
///     address->AccountChangeSet (It contains old and new account info,storage wipe flag, and
/// old/new storage)     bytecode_hash->bytecodes mapping
///
/// BTreeMap is used to have sorted values
pub fn commit_changes<DB: StateProvider>(
    db: &mut SubState<DB>,
    changes: hashbrown::HashMap<Address, RevmAccount>,
) -> (BTreeMap<Address, AccountChangeSet>, BTreeMap<H256, Bytecode>) {
    let mut change = BTreeMap::new();
    let mut new_bytecodes = BTreeMap::new();
    // iterate over all changed accounts
    for (address, account) in changes {
        if account.is_destroyed {
            // get old account that we are destroying.
            let db_account = match db.accounts.entry(address) {
                Entry::Occupied(entry) => entry.into_mut(),
                Entry::Vacant(_entry) => {
                    panic!("Left panic to critically jumpout if happens as this should not hapen.");
                }
            };
            // Insert into `change` a old account and None for new account
            // and mark storage to be mapped
            change.insert(
                address,
                AccountChangeSet {
                    account: (
                        Some(Account {
                            nonce: db_account.info.nonce,
                            balance: db_account.info.balance,
                            bytecode_hash: Some(db_account.info.code_hash), /* TODO none if
                                                                             * KECCAK_EMPTY */
                        }),
                        None,
                    ),
                    storage: BTreeMap::new(),
                    wipe_storage: true,
                },
            );
            // clear cached DB and mark account as not existing
            db_account.storage.clear();
            db_account.account_state = AccountState::NotExisting;
            db_account.info = AccountInfo::default();

            continue
        } else {
            // check if account code is new or old.
            // does it exist inside cached contracts if it doesn't it is new bytecode that
            // we are inserting inside `change`
            if let Some(ref code) = account.info.code {
                if !code.is_empty() {
                    match db.contracts.entry(account.info.code_hash) {
                        Entry::Vacant(entry) => {
                            entry.insert(code.clone());
                            new_bytecodes.insert(account.info.code_hash, code.clone());
                        }
                        Entry::Occupied(mut entry) => {
                            entry.insert(code.clone());
                        }
                    }
                }
            }

            // get old account that is going to be overwritten or none if it does not exist
            // and get new account that was just inserted. new account mut ref is used for
            // inserting storage
            let (old_account, has_change, new_account) = match db.accounts.entry(address) {
                Entry::Vacant(entry) => {
                    let entry = entry.insert(Default::default());
                    entry.info = account.info.clone();
                    (None, true, entry)
                }
                Entry::Occupied(entry) => {
                    let entry = entry.into_mut();
                    let (old_account, has_change) =
                        if matches!(entry.account_state, AccountState::NotExisting) {
                            (None, true)
                        } else {
                            (Some(entry.info.clone()), entry.info != account.info)
                        };
                    entry.info = account.info.clone();
                    (old_account, has_change, entry)
                }
            };
            // cast from revm type to reth type
            let old_account = old_account.map(|acc| Account {
                balance: acc.balance,
                nonce: acc.nonce,
                bytecode_hash: Some(acc.code_hash), // TODO if KECCAK_EMPTY return None
            });
            let mut wipe_storage = false;

            new_account.account_state = if account.storage_cleared {
                new_account.storage.clear();
                wipe_storage = true;
                AccountState::StorageCleared
            } else {
                AccountState::Touched
            };

            // Insert storage.
            let mut storage = BTreeMap::new();

            // insert storage into new db account.
            new_account.storage.extend(account.storage.into_iter().map(|(key, value)| {
                storage.insert(key, (value.original_value(), value.present_value()));
                (key, value.present_value())
            }));

            // Insert into change.
            change.insert(
                address,
                AccountChangeSet {
                    account: if has_change {
                        (
                            old_account,
                            Some(new_account).map(|acc| {
                                Account {
                                    balance: acc.info.balance,
                                    nonce: acc.info.nonce,
                                    bytecode_hash: Some(acc.info.code_hash), // TODO if KECCAK_EMPTY return None
                                }
                            }),
                        )
                    } else {
                        (None, None)
                    },
                    storage,
                    wipe_storage,
                },
            );
        }
    }
    (change, new_bytecodes)
}

/// After transaction is executed this structure contain
/// every change to state that this transaction made and its old values
/// so that history account table can be updated. Receipts and new bytecodes
/// from created bytecodes.
#[derive(Debug, Clone)]
pub struct TransactionStatePatch {
    /// Transaction receipt
    pub receipt: Receipt,
    /// State change that this transaction made on state.
    pub state_diff: BTreeMap<Address, AccountChangeSet>,
    /// new bytecode created as result of transaction execution.
    pub new_bytecodes: BTreeMap<H256, Bytecode>,
}

/// Execute and verify block
pub fn execute_and_verify_receipt<DB: StateProvider>(
    header: &SealedHeader,
    transactions: &[TransactionSignedEcRecovered],
    config: &Config,
    db: &mut SubState<DB>,
) -> Result<Vec<TransactionStatePatch>, Error> {
    let transaction_patches = execute(header, transactions, config, db)?;

    let receipts_iter = transaction_patches.iter().map(|patch| &patch.receipt);
    verify_receipt(header.receipts_root, header.logs_bloom, receipts_iter)?;

    Ok(transaction_patches)
}

/// Verify receipts
pub fn verify_receipt<'a>(
    expected_receipts_root: H256,
    expected_logs_bloom: Bloom,
    receipts: impl Iterator<Item = &'a Receipt> + Clone,
) -> Result<(), Error> {
    // Check receipts root.
    let receipts_root = reth_primitives::proofs::calculate_receipt_root(receipts.clone());
    if receipts_root != expected_receipts_root {
        return Err(Error::ReceiptRootDiff { got: receipts_root, expected: expected_receipts_root })
    }

    // Create header log bloom.
    let logs_bloom = receipts.fold(Bloom::zero(), |bloom, r| bloom | r.bloom);
    if logs_bloom != expected_logs_bloom {
        return Err(Error::BloomLogDiff {
            expected: Box::new(expected_logs_bloom),
            got: Box::new(logs_bloom),
        })
    }
    Ok(())
}

/// Verify block. Execute all transaction and compare results.
/// Return diff is on transaction granularity. We are returning vector of
pub fn execute<DB: StateProvider>(
    header: &SealedHeader,
    transactions: &[TransactionSignedEcRecovered],
    config: &Config,
    db: &mut SubState<DB>,
) -> Result<Vec<TransactionStatePatch>, Error> {
    let mut evm = EVM::new();
    evm.database(db);

    evm.env.cfg.chain_id = config.chain_id;
    evm.env.cfg.spec_id = config.spec_upgrades.revm_spec(header.number);
    evm.env.cfg.perf_all_precompiles_have_balance = true;
    evm.env.cfg.perf_analyse_created_bytecodes = AnalysisKind::Raw;

    revm_wrap::fill_block_env(&mut evm.env.block, header);
    let mut cumulative_gas_used = 0;
    // output of verification
    let mut transaction_patch = Vec::with_capacity(transactions.len());

    for transaction in transactions.iter() {
        // The sum of the transaction’s gas limit, Tg, and the gas utilised in this block prior,
        // must be no greater than the block’s gasLimit.
        let block_available_gas = header.gas_limit - cumulative_gas_used;
        if transaction.gas_limit() > block_available_gas {
            return Err(Error::TransactionGasLimitMoreThenAvailableBlockGas {
                transaction_gas_limit: transaction.gas_limit(),
                block_available_gas,
            })
        }

        // Fill revm structure.
        revm_wrap::fill_tx_env(&mut evm.env.tx, transaction);

        // Execute transaction.
        let (ExecutionResult { exit_reason, gas_used, logs, .. }, state) = evm.transact();

        // Fatal internal error.
        if exit_reason == revm::Return::FatalExternalError {
            return Err(Error::ExecutionFatalError)
        }

        // Success flag was added in `EIP-658: Embedding transaction status code in receipts`.
        let is_success = matches!(
            exit_reason,
            revm::Return::Continue |
                revm::Return::Stop |
                revm::Return::Return |
                revm::Return::SelfDestruct
        );

        // Add spend gas.
        cumulative_gas_used += gas_used;

        // Transform logs to reth format.
        let logs: Vec<Log> = logs
            .into_iter()
            .map(|l| Log { address: l.address, topics: l.topics, data: l.data })
            .collect();

        // commit state
        let (state_diff, new_bytecodes) = commit_changes(evm.db().unwrap(), state);

        // Push transaction patch and calculte header bloom filter for receipt.
        transaction_patch.push(TransactionStatePatch {
            receipt: Receipt {
                tx_type: transaction.tx_type(),
                success: is_success,
                cumulative_gas_used,
                bloom: logs_bloom(logs.iter()),
                logs,
            },
            state_diff,
            new_bytecodes,
        })
    }

    // Check if gas used matches the value set in header.
    if header.gas_used != cumulative_gas_used {
        return Err(Error::BlockGasUsed { got: cumulative_gas_used, expected: header.gas_used })
    }

    // TODO add validator block reward. Currently not added.
    // https://github.com/foundry-rs/reth/issues/237

    Ok(transaction_patch)
}

#[cfg(test)]
mod tests {

    use std::collections::HashMap;

    use crate::revm_wrap::State;
    use reth_interfaces::provider::{AccountProvider, StateProvider};
    use reth_primitives::{
        hex_literal::hex, keccak256, Account, Address, BlockLocked, Bytes, StorageKey, H160, H256,
        U256,
    };
    use reth_rlp::Decodable;

    use super::*;

    #[derive(Debug, Default, Clone, Eq, PartialEq)]
    struct StateProviderTest {
        accounts: HashMap<Address, (HashMap<StorageKey, U256>, Account)>,
        contracts: HashMap<H256, Bytes>,
        block_hash: HashMap<U256, H256>,
    }

    impl StateProviderTest {
        /// Insert account.
        fn insert_account(
            &mut self,
            address: Address,
            mut account: Account,
            bytecode: Option<Bytes>,
            storage: HashMap<StorageKey, U256>,
        ) {
            if let Some(bytecode) = bytecode {
                let hash = keccak256(&bytecode);
                account.bytecode_hash = Some(hash);
                self.contracts.insert(hash, bytecode);
            }
            self.accounts.insert(address, (storage, account));
        }
    }

    impl AccountProvider for StateProviderTest {
        fn basic_account(&self, address: Address) -> reth_interfaces::Result<Option<Account>> {
            let ret = Ok(self.accounts.get(&address).map(|(_, acc)| *acc));
            ret
        }
    }

    impl StateProvider for StateProviderTest {
        fn storage(
            &self,
            account: Address,
            storage_key: reth_primitives::StorageKey,
        ) -> reth_interfaces::Result<Option<reth_primitives::StorageValue>> {
            Ok(self
                .accounts
                .get(&account)
                .and_then(|(storage, _)| storage.get(&storage_key).cloned()))
        }

        fn bytecode_by_hash(&self, code_hash: H256) -> reth_interfaces::Result<Option<Bytes>> {
            Ok(self.contracts.get(&code_hash).cloned())
        }

        fn block_hash(&self, number: U256) -> reth_interfaces::Result<Option<H256>> {
            Ok(self.block_hash.get(&number).cloned())
        }
    }

    #[test]
    fn sanity_execution() {
        // Got rlp block from: src/GeneralStateTestsFiller/stChainId/chainIdGasCostFiller.json

        let mut block_rlp = hex!("f90262f901f9a075c371ba45999d87f4542326910a11af515897aebce5265d3f6acd1f1161f82fa01dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347942adc25665018aa1fe0e6bc666dac8fc2697ff9baa098f2dcd87c8ae4083e7017a05456c14eea4b1db2032126e27b3b1563d57d7cc0a08151d548273f6683169524b66ca9fe338b9ce42bc3540046c828fd939ae23bcba03f4e5c2ec5b2170b711d97ee755c160457bb58d8daa338e835ec02ae6860bbabb901000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000083020000018502540be40082a8798203e800a00000000000000000000000000000000000000000000000000000000000000000880000000000000000f863f861800a8405f5e10094100000000000000000000000000000000000000080801ba07e09e26678ed4fac08a249ebe8ed680bf9051a5e14ad223e4b2b9d26e0208f37a05f6e3f188e3e6eab7d7d3b6568f5eac7d687b08d307d3154ccd8c87b4630509bc0").as_slice();
        let block = BlockLocked::decode(&mut block_rlp).unwrap();

        let mut db = StateProviderTest::default();

        // pre staet
        db.insert_account(
            H160(hex!("1000000000000000000000000000000000000000")),
            Account { balance: 0x00.into(), nonce: 0x00, bytecode_hash: None },
            Some(hex!("5a465a905090036002900360015500").into()),
            HashMap::new(),
        );

        let account3_old_info = Account {
            balance: 0x3635c9adc5dea00000u128.into(),
            nonce: 0x00,
            bytecode_hash: Some(H256(hex!(
                "c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470"
            ))),
        };

        db.insert_account(
            H160(hex!("a94f5374fce5edbc8e2a8697c15331677e6ebf0b")),
            Account { balance: 0x3635c9adc5dea00000u128.into(), nonce: 0x00, bytecode_hash: None },
            None,
            HashMap::new(),
        );

        let mut config = Config::new_ethereum();
        // make it berlin fork
        config.spec_upgrades.berlin = 0;

        let mut db = SubState::new(State::new(db));
        let transactions: Vec<TransactionSignedEcRecovered> =
            block.body.iter().map(|tx| tx.try_ecrecovered().unwrap()).collect();

        // execute chain and verify receipts
        let out =
            execute_and_verify_receipt(&block.header, &transactions, &config, &mut db).unwrap();

        assert_eq!(out.len(), 1, "Should executed one transaction");

        let patch = out[0].clone();
        assert_eq!(patch.new_bytecodes.len(), 0, "Should have zero new bytecodes");

        let account1 = H160(hex!("1000000000000000000000000000000000000000"));
        let _account1_info = Account {
            balance: 0x00.into(),
            nonce: 0x00,
            bytecode_hash: Some(H256(hex!(
                "c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470"
            ))),
        };
        let account2 = H160(hex!("2adc25665018aa1fe0e6bc666dac8fc2697ff9ba"));
        let account2_info = Account {
            balance: (0x1bc16d674ece94bau128 - 0x1bc16d674ec80000u128).into(), /* TODO remove
                                                                                * 2eth block
                                                                                * reward */
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

        assert_eq!(
            patch.state_diff.get(&account1).unwrap().account,
            (None, None),
            "No change to account"
        );
        assert_eq!(
            patch.state_diff.get(&account2).unwrap().account,
            (None, Some(account2_info)),
            "New acccount"
        );
        assert_eq!(
            patch.state_diff.get(&account3).unwrap().account,
            (Some(account3_old_info), Some(account3_info)),
            "Change to account state"
        );

        assert_eq!(patch.new_bytecodes.len(), 0, "No new bytecodes");

        // check torage
        let storage = &patch.state_diff.get(&account1).unwrap().storage;
        assert_eq!(storage.len(), 1, "Only one storage change");
        assert_eq!(
            storage.get(&1.into()),
            Some(&(0.into(), 2.into())),
            "Storage change from 0 to 2 on slot 1"
        );
    }
}
