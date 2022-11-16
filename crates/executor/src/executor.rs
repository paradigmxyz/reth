use crate::{
    revm_wrap::{self, State, SubState},
    Config,
};
use hashbrown::hash_map::Entry;
use reth_interfaces::{
    executor::{BlockExecutor, Error},
    provider::StateProvider,
};
use reth_primitives::{
    bloom::logs_bloom, Account, Address, BlockLocked, Bloom, Log, Receipt, H160, H256, U256,
};
use revm::{
    db::AccountState, Account as RevmAccount, AccountInfo, AnalysisKind, Bytecode, ExecutionResult,
    SpecId, EVM,
};
use std::collections::{BTreeMap, HashMap};

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
    /// In most cases we would have (Some,Some)
    pub account: (Option<Account>, Option<Account>),
    /// Storage containing key -> (OldValue,NewValue). in case that old value is not existing
    /// we can expect to have U256::zero(), same with new value.
    pub storage: BTreeMap<U256, (U256, U256)>,
    /// Just to make sure that we are taking selfdestruct cleaning we have this field that wipes storage.
    /// There are instances where storage is changed but account is not touched, so we can't take into account
    /// that if new account is None that it is selfdestruct.
    pub wipe_storage: bool,
}

/// Commit chgange to database and return change diff that is used to update state and create history index
///
pub fn commit_changes<DB: StateProvider>(
    db: &mut SubState<DB>,
    changes: HashMap<Address, RevmAccount>,
) -> (HashMap<Address, AccountChangeSet>, HashMap<H256, Bytecode>) {
    let mut change = HashMap::new();
    let mut new_bytecodes = HashMap::new();
    for (address, mut account) in changes {
        if account.is_destroyed {
            let db_account = match db.accounts.entry(address) {
                Entry::Occupied(entry) => entry.into_mut(),
                Entry::Vacant(_entry) => {
                    panic!("Left panic to critically jumpout if happens as this should not hapen.");
                }
            };
            change.insert(
                address,
                AccountChangeSet {
                    account: (
                        Some(Account {
                            nonce: db_account.info.nonce,
                            balance: db_account.info.balance,
                            bytecode_hash: Some(db_account.info.code_hash), // TODO none if KECCAK_EMPTY
                        }),
                        None,
                    ),
                    storage: BTreeMap::new(),
                    wipe_storage: true,
                },
            );

            db_account.storage.clear();
            db_account.account_state = AccountState::NotExisting;
            db_account.info = AccountInfo::default();

            continue;
        } else {
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

            // get account
            let (old_account, new_account) = match db.accounts.entry(address) {
                Entry::Vacant(mut entry) => {
                    let entry = entry.insert(Default::default());
                    entry.info = account.info.clone();
                    (None, entry)
                }
                Entry::Occupied(entry) => {
                    let entry = entry.into_mut();
                    let old_account = Some(entry.info.clone());
                    entry.info = account.info.clone();
                    (old_account, entry)
                }
            };

            let db_account = db.accounts.entry(address).or_default();
            db_account.info = account.info.clone();

            db_account.account_state = if account.storage_cleared {
                db_account.storage.clear();
                AccountState::StorageCleared
            } else {
                AccountState::Touched
            };
            db_account.storage.extend(
                account.storage.into_iter().map(|(key, value)| (key, value.present_value())),
            );
        }
    }
    (change, new_bytecodes)
}

impl Executor {
    /// Create new Executor
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    /// Verify block. Execute all transaction and compare results.
    pub fn verify<DB: StateProvider>(
        &self,
        block: &BlockLocked,
        confg: &Config,
        db: &mut SubState<DB>,
    ) -> Result<Vec<Receipt>, Error> {
        //let db = SubState::new(State::new(db));
        let mut evm = EVM::new();
        evm.database(db);

        evm.env.cfg.chain_id = 1.into();
        evm.env.cfg.spec_id = SpecId::LATEST;
        evm.env.cfg.perf_all_precompiles_have_balance = true;
        evm.env.cfg.perf_analyse_created_bytecodes = AnalysisKind::Raw;

        revm_wrap::fill_block_env(&mut evm.env.block, block);
        let mut cumulative_gas_used = 0;
        let mut receipts = Vec::with_capacity(block.body.len());
        //let mut state_diff = HashMap::new();

        for transaction in block.body.iter() {
            // The sum of the transaction’s gas limit, Tg, and the gas utilised in this block prior,
            // must be no greater than the block’s gasLimit.
            let block_available_gas = block.gas_limit - cumulative_gas_used;
            if transaction.gas_limit() > block_available_gas {
                return Err(Error::TransactionGasLimitMoreThenAvailableBlockGas {
                    transaction_gas_limit: transaction.gas_limit(),
                    block_available_gas,
                });
            }

            // Fill revm structure.
            revm_wrap::fill_tx_env(&mut evm.env.tx, transaction.as_ref());

            // Execute transaction.
            let (ExecutionResult { exit_reason, gas_used, logs, .. }, state) = evm.transact();

            for account in state {}

            // Fatal internal error.
            if exit_reason == revm::Return::FatalExternalError {
                return Err(Error::ExecutionFatalError);
            }

            // Success flag was added in `EIP-658: Embedding transaction status code in receipts`.
            let is_success = matches!(
                exit_reason,
                revm::Return::Continue
                    | revm::Return::Stop
                    | revm::Return::Return
                    | revm::Return::SelfDestruct
            );

            // Add spend gas.
            cumulative_gas_used += gas_used;

            // Transform logs to reth format.
            let logs: Vec<Log> = logs
                .into_iter()
                .map(|l| Log { address: l.address, topics: l.topics, data: l.data })
                .collect();

            // Push receipts for calculating header bloom filter.
            receipts.push(Receipt {
                tx_type: transaction.tx_type(),
                success: is_success,
                cumulative_gas_used,
                bloom: logs_bloom(logs.iter()), // TODO
                logs,
            });
        }

        // TODO do state root.

        // Check if gas used matches the value set in header.
        if block.gas_used != cumulative_gas_used {
            return Err(Error::BlockGasUsed { got: cumulative_gas_used, expected: block.gas_used });
        }

        // Check receipts root.
        let receipts_root = reth_primitives::proofs::calculate_receipt_root(receipts.iter());
        if block.receipts_root != receipts_root {
            return Err(Error::ReceiptRootDiff {
                got: receipts_root,
                expected: block.receipts_root,
            });
        }

        // Create header log bloom.
        let expected_logs_bloom = receipts.iter().fold(Bloom::zero(), |bloom, r| bloom | r.bloom);
        if expected_logs_bloom != block.logs_bloom {
            return Err(Error::BloomLogDiff {
                expected: Box::new(block.logs_bloom),
                got: Box::new(expected_logs_bloom),
            });
        }

        Ok(receipts)
    }
}

impl BlockExecutor for Executor {}
