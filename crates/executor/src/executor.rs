use crate::{
    revm_wrap::{self, SubState},
    Config,
};
use hashbrown::hash_map::Entry;
use reth_interfaces::{executor::Error, provider::StateProvider};
use reth_primitives::{
    bloom::logs_bloom, Account, Address, BlockLocked, Bloom, Log, Receipt, H256, U256,
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
                    let old_account = Some(entry.info.clone());
                    let has_change = entry.info != account.info;
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
                storage.insert(key.clone(), (value.original_value(), value.present_value()));
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
    block: &BlockLocked,
    config: &Config,
    db: &mut SubState<DB>,
) -> Result<Vec<TransactionStatePatch>, Error> {
    let transaction_patches = execute(block, config, db)?;

    let receipts_iter = transaction_patches.iter().map(|patch| &patch.receipt);
    verify_receipt(block.receipts_root, block.logs_bloom, receipts_iter)?;

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
    block: &BlockLocked,
    config: &Config,
    db: &mut SubState<DB>,
) -> Result<Vec<TransactionStatePatch>, Error> {
    let mut evm = EVM::new();
    evm.database(db);

    evm.env.cfg.chain_id = config.chain_id;
    evm.env.cfg.spec_id = config.spec_upgrades.revm_spec(block.number);
    evm.env.cfg.perf_all_precompiles_have_balance = true;
    evm.env.cfg.perf_analyse_created_bytecodes = AnalysisKind::Raw;

    revm_wrap::fill_block_env(&mut evm.env.block, block);
    let mut cumulative_gas_used = 0;
    // output of verification
    let mut transaction_patch = Vec::with_capacity(block.body.len());

    for transaction in block.body.iter() {
        // The sum of the transaction’s gas limit, Tg, and the gas utilised in this block prior,
        // must be no greater than the block’s gasLimit.
        let block_available_gas = block.gas_limit - cumulative_gas_used;
        if transaction.gas_limit() > block_available_gas {
            return Err(Error::TransactionGasLimitMoreThenAvailableBlockGas {
                transaction_gas_limit: transaction.gas_limit(),
                block_available_gas,
            })
        }

        // Fill revm structure.
        revm_wrap::fill_tx_env(&mut evm.env.tx, transaction.as_ref());

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
    if block.gas_used != cumulative_gas_used {
        return Err(Error::BlockGasUsed { got: cumulative_gas_used, expected: block.gas_used })
    }

    Ok(transaction_patch)
}
