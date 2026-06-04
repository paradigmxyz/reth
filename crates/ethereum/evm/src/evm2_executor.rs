//! Evm2-backed Ethereum execution helpers.

use crate::{evm2_recovered_tx, RethReceiptBuilder};
use alloc::{format, string::String, vec::Vec};
use alloy_consensus::transaction::Recovered;
use alloy_eips::eip4895::Withdrawal;
use alloy_primitives::{Address, Bytes, B256, KECCAK256_EMPTY, U256};
use evm2::{
    env::BlockEnv,
    ethereum::ethereum_tx_registry,
    evm::{
        AccountInfo, Database, Db, DbErrorCode, StateChanges, Tracked, BEACON_ROOTS_ADDRESS,
        HISTORY_STORAGE_ADDRESS,
    },
    registry::HandlerError,
    BaseEvmTypes, Evm, Precompiles, SpecId,
};
use reth_ethereum_primitives::{Receipt, TransactionSigned};
use reth_execution_types::{BlockExecutionOutput, Evm2BundleState};
#[cfg(feature = "std")]
use reth_storage_api::{Evm2StateProviderDatabase, StateProvider};
#[cfg(feature = "std")]
use reth_storage_errors::provider::ProviderError;

/// Error returned by evm2-backed Ethereum execution.
#[derive(Debug)]
pub enum Evm2ExecutionError<E> {
    /// Evm2 rejected or halted transaction execution before producing a Reth output.
    Handler(HandlerError),
    /// Evm2 reported a database error and the typed database error was available.
    Database(E),
    /// Evm2 reported a database error, but the typed database error was no longer available.
    MissingDatabaseError(DbErrorCode),
    /// Cancun requires a parent beacon block root after genesis.
    MissingParentBeaconBlockRoot,
    /// Cancun genesis payloads must carry a zero parent beacon block root.
    CancunGenesisParentBeaconBlockRootNotZero(B256),
    /// A pre-block system call reverted or halted without producing a successful result.
    SystemCallFailed {
        /// System contract address that was called.
        address: Address,
        /// Evm2 stop reason for the failed call.
        reason: String,
    },
}

impl<E> core::fmt::Display for Evm2ExecutionError<E>
where
    E: core::fmt::Display,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Handler(err) => write!(f, "evm2 execution error: {err}"),
            Self::Database(err) => write!(f, "evm2 database error: {err}"),
            Self::MissingDatabaseError(code) => {
                write!(f, "evm2 database error {code:?} was not available")
            }
            Self::MissingParentBeaconBlockRoot => {
                f.write_str("missing parent beacon block root for Cancun system call")
            }
            Self::CancunGenesisParentBeaconBlockRootNotZero(root) => {
                write!(f, "Cancun genesis parent beacon block root must be zero, got {root}")
            }
            Self::SystemCallFailed { address, reason } => {
                write!(f, "evm2 system call to {address} failed: {reason}")
            }
        }
    }
}

impl<E> core::error::Error for Evm2ExecutionError<E> where E: core::error::Error + Send + 'static {}

/// Additional block-level execution context for evm2.
#[derive(Debug, Clone, Copy, Default)]
pub struct Evm2BlockExecutionContext<'a> {
    /// Pre-block system calls to run before transaction execution.
    pub system_calls: Option<Evm2BlockSystemCalls>,
    /// Post-block withdrawals to apply after transaction execution.
    pub withdrawals: Option<&'a [Withdrawal]>,
}

/// Inputs required by Ethereum pre-block system calls.
#[derive(Debug, Clone, Copy)]
pub struct Evm2BlockSystemCalls {
    /// Parent block hash for EIP-2935 history storage.
    pub parent_hash: B256,
    /// Parent beacon block root for EIP-4788 beacon roots.
    pub parent_beacon_block_root: Option<B256>,
}

/// Executes a block worth of recovered Ethereum transactions with evm2.
pub fn execute_evm2_block<DB>(
    spec_id: SpecId,
    block_env: BlockEnv,
    database: DB,
    block_number: u64,
    transactions: impl IntoIterator<Item = Recovered<TransactionSigned>>,
) -> Result<BlockExecutionOutput<Receipt>, Evm2ExecutionError<DB::Error>>
where
    DB: Database + 'static,
{
    execute_evm2_block_with_withdrawals(
        spec_id,
        block_env,
        database,
        block_number,
        transactions,
        None,
    )
}

/// Executes a block worth of recovered Ethereum transactions and post-block withdrawals with evm2.
pub fn execute_evm2_block_with_withdrawals<DB>(
    spec_id: SpecId,
    block_env: BlockEnv,
    database: DB,
    block_number: u64,
    transactions: impl IntoIterator<Item = Recovered<TransactionSigned>>,
    withdrawals: Option<&[Withdrawal]>,
) -> Result<BlockExecutionOutput<Receipt>, Evm2ExecutionError<DB::Error>>
where
    DB: Database + 'static,
{
    execute_evm2_block_with_context(
        spec_id,
        block_env,
        database,
        block_number,
        transactions,
        Evm2BlockExecutionContext { system_calls: None, withdrawals },
    )
}

/// Executes a block worth of recovered Ethereum transactions with additional block-level context.
pub fn execute_evm2_block_with_context<DB>(
    spec_id: SpecId,
    block_env: BlockEnv,
    database: DB,
    block_number: u64,
    transactions: impl IntoIterator<Item = Recovered<TransactionSigned>>,
    context: Evm2BlockExecutionContext<'_>,
) -> Result<BlockExecutionOutput<Receipt>, Evm2ExecutionError<DB::Error>>
where
    DB: Database + 'static,
{
    let mut evm = Evm::<BaseEvmTypes>::new(
        spec_id,
        block_env,
        ethereum_tx_registry(spec_id),
        Db::new(database),
        Precompiles::base(spec_id),
    );
    let pre_system_changes =
        pre_execution_system_call_state_changes::<DB>(&mut evm, spec_id, block_number, context)?;
    let mut results = Vec::new();

    for transaction in transactions {
        let tx_type = transaction.inner().tx_type();
        let transaction = evm2_recovered_tx(transaction);
        let result =
            evm.transact(&transaction).map_err(|err| map_handler_error::<DB>(&mut evm, err))?;

        if let Some(code) = result.db_error_code {
            return Err(take_database_error::<DB>(&mut evm)
                .map(Evm2ExecutionError::Database)
                .unwrap_or(Evm2ExecutionError::MissingDatabaseError(code)))
        }

        results.push((tx_type, result));
    }

    let withdrawal_changes =
        withdrawal_state_changes::<DB>(&mut evm, block_number, &results, context.withdrawals)?;

    Ok(RethReceiptBuilder.build_evm2_block_output_with_surrounding_state_changes(
        block_number,
        pre_system_changes,
        results,
        withdrawal_changes,
    ))
}

/// Executes a block worth of recovered Ethereum transactions with an evm2 database adapter backed
/// by a Reth state provider.
#[cfg(feature = "std")]
pub fn execute_evm2_block_with_state_provider<DB>(
    spec_id: SpecId,
    block_env: BlockEnv,
    state_provider: DB,
    block_number: u64,
    transactions: impl IntoIterator<Item = Recovered<TransactionSigned>>,
) -> Result<BlockExecutionOutput<Receipt>, Evm2ExecutionError<ProviderError>>
where
    DB: StateProvider + Send + 'static,
{
    execute_evm2_block(
        spec_id,
        block_env,
        Evm2StateProviderDatabase::new(state_provider),
        block_number,
        transactions,
    )
}

/// Executes a block worth of recovered Ethereum transactions and withdrawals with an evm2 database
/// adapter backed by a Reth state provider.
#[cfg(feature = "std")]
pub fn execute_evm2_block_with_state_provider_and_withdrawals<DB>(
    spec_id: SpecId,
    block_env: BlockEnv,
    state_provider: DB,
    block_number: u64,
    transactions: impl IntoIterator<Item = Recovered<TransactionSigned>>,
    withdrawals: Option<&[Withdrawal]>,
) -> Result<BlockExecutionOutput<Receipt>, Evm2ExecutionError<ProviderError>>
where
    DB: StateProvider + Send + 'static,
{
    execute_evm2_block_with_withdrawals(
        spec_id,
        block_env,
        Evm2StateProviderDatabase::new(state_provider),
        block_number,
        transactions,
        withdrawals,
    )
}

/// Executes a block worth of recovered Ethereum transactions with additional block-level context
/// and an evm2 database adapter backed by a Reth state provider.
#[cfg(feature = "std")]
pub fn execute_evm2_block_with_state_provider_context<DB>(
    spec_id: SpecId,
    block_env: BlockEnv,
    state_provider: DB,
    block_number: u64,
    transactions: impl IntoIterator<Item = Recovered<TransactionSigned>>,
    context: Evm2BlockExecutionContext<'_>,
) -> Result<BlockExecutionOutput<Receipt>, Evm2ExecutionError<ProviderError>>
where
    DB: StateProvider + Send + 'static,
{
    execute_evm2_block_with_context(
        spec_id,
        block_env,
        Evm2StateProviderDatabase::new(state_provider),
        block_number,
        transactions,
        context,
    )
}

fn map_handler_error<DB>(
    evm: &mut Evm<BaseEvmTypes>,
    err: HandlerError,
) -> Evm2ExecutionError<DB::Error>
where
    DB: Database + 'static,
{
    match err {
        HandlerError::Database(code) => take_database_error::<DB>(evm)
            .map(Evm2ExecutionError::Database)
            .unwrap_or(Evm2ExecutionError::MissingDatabaseError(code)),
        err => Evm2ExecutionError::Handler(err),
    }
}

fn take_database_error<DB>(evm: &mut Evm<BaseEvmTypes>) -> Option<DB::Error>
where
    DB: Database + 'static,
{
    evm.database_as_mut::<Db<DB>>().and_then(Db::take_result)
}

fn pre_execution_system_call_state_changes<DB>(
    evm: &mut Evm<BaseEvmTypes>,
    spec_id: SpecId,
    block_number: u64,
    context: Evm2BlockExecutionContext<'_>,
) -> Result<Vec<StateChanges>, Evm2ExecutionError<DB::Error>>
where
    DB: Database + 'static,
{
    let mut state_changes = Vec::new();
    let Some(system_calls) = context.system_calls else {
        return Ok(state_changes);
    };

    if spec_id.enables(SpecId::PRAGUE) && block_number != 0 {
        state_changes.push(execute_system_call::<DB>(
            evm,
            HISTORY_STORAGE_ADDRESS,
            system_calls.parent_hash.0.into(),
        )?);
    }

    if spec_id.enables(SpecId::CANCUN) {
        let parent_beacon_block_root = system_calls
            .parent_beacon_block_root
            .ok_or(Evm2ExecutionError::MissingParentBeaconBlockRoot)?;

        if block_number == 0 {
            if parent_beacon_block_root != B256::ZERO {
                return Err(Evm2ExecutionError::CancunGenesisParentBeaconBlockRootNotZero(
                    parent_beacon_block_root,
                ));
            }
        } else {
            state_changes.push(execute_system_call::<DB>(
                evm,
                BEACON_ROOTS_ADDRESS,
                parent_beacon_block_root.0.into(),
            )?);
        }
    }

    Ok(state_changes)
}

fn execute_system_call<DB>(
    evm: &mut Evm<BaseEvmTypes>,
    address: Address,
    data: Bytes,
) -> Result<StateChanges, Evm2ExecutionError<DB::Error>>
where
    DB: Database + 'static,
{
    let result = evm.system_call(address, data);

    if let Some(code) = result.db_error_code {
        return Err(take_database_error::<DB>(evm)
            .map(Evm2ExecutionError::Database)
            .unwrap_or(Evm2ExecutionError::MissingDatabaseError(code)))
    }

    if !result.status {
        return Err(Evm2ExecutionError::SystemCallFailed {
            address,
            reason: format!("{:?}", result.stop),
        });
    }

    Ok(result.state_changes)
}

fn withdrawal_state_changes<DB>(
    evm: &mut Evm<BaseEvmTypes>,
    block_number: u64,
    txs: &[(alloy_consensus::TxType, evm2::TxResult)],
    withdrawals: Option<&[Withdrawal]>,
) -> Result<Option<StateChanges>, Evm2ExecutionError<DB::Error>>
where
    DB: Database + 'static,
{
    let Some(withdrawals) = withdrawals.filter(|withdrawals| !withdrawals.is_empty()) else {
        return Ok(None);
    };

    let mut bundle = Evm2BundleState::new(block_number);
    bundle.append_block(txs.iter().map(|(_, result)| result.state_changes.clone()));

    let db =
        evm.database_as_mut::<Db<DB>>().expect("evm database should use the typed evm2 Db adapter");
    let mut changes = StateChanges::default();

    for withdrawal in withdrawals {
        let (original, mut current) = match changes.accounts.get(&withdrawal.address) {
            Some(account) => {
                (account.original.clone(), account.current.clone().unwrap_or_else(empty_account))
            }
            None => {
                let original = match bundle.accounts().get(&withdrawal.address) {
                    Some(account) => account.current.clone(),
                    None => db
                        .inner_mut()
                        .get_account(&withdrawal.address)
                        .map_err(Evm2ExecutionError::Database)?,
                };
                let current = original.clone().unwrap_or_else(empty_account);
                (original, current)
            }
        };

        current.balance = current.balance.saturating_add(withdrawal.amount_wei());
        changes.accounts.insert(
            withdrawal.address,
            Tracked { original, current: Some(current), _non_exhaustive: () },
        );
    }

    Ok(Some(changes))
}

fn empty_account() -> AccountInfo {
    AccountInfo {
        balance: U256::ZERO,
        nonce: 0,
        code_hash: KECCAK256_EMPTY,
        code: None,
        _non_exhaustive: (),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::collections::BTreeMap;
    use alloy_consensus::{SignableTransaction, TxLegacy};
    use alloy_eips::eip4895::Withdrawal;
    use alloy_primitives::{address, Address, Bytes, Signature, TxKind, B256, U256};
    use core::convert::Infallible;
    use evm2::{bytecode::Bytecode, evm::AccountInfo, interpreter::Word};

    #[derive(Default)]
    struct TestDatabase {
        accounts: BTreeMap<Address, AccountInfo>,
    }

    impl Database for TestDatabase {
        type Error = Infallible;

        fn get_account(&mut self, address: &Address) -> Result<Option<AccountInfo>, Self::Error> {
            Ok(self.accounts.get(address).cloned())
        }

        fn get_code_by_hash(&mut self, _code_hash: &B256) -> Result<Bytecode, Self::Error> {
            Ok(Bytecode::default())
        }

        fn get_storage(&mut self, _address: &Address, _key: &Word) -> Result<Word, Self::Error> {
            Ok(Word::ZERO)
        }

        fn get_block_hash(&mut self, _number: &Word) -> Result<Option<B256>, Self::Error> {
            Ok(None)
        }
    }

    #[test]
    fn executes_legacy_transfer_with_evm2() {
        let caller = address!("0000000000000000000000000000000000000001");
        let target = address!("0000000000000000000000000000000000001000");
        let mut database = TestDatabase::default();
        database
            .accounts
            .insert(caller, AccountInfo::default().with_balance(U256::from(1_000_000u64)));
        let transaction = Recovered::new_unchecked(
            TransactionSigned::Legacy(
                TxLegacy {
                    gas_price: 1,
                    gas_limit: 21_000,
                    to: TxKind::Call(target),
                    value: U256::from(1),
                    input: Bytes::new(),
                    ..Default::default()
                }
                .into_signed(Signature::test_signature()),
            ),
            caller,
        );

        let output =
            execute_evm2_block(SpecId::FRONTIER, BlockEnv::default(), database, 1, [transaction])
                .expect("evm2 execution succeeds");

        assert_eq!(output.result.gas_used, 21_000);
        assert_eq!(output.result.receipts.len(), 1);
        assert!(output.result.receipts[0].success);
        assert_eq!(
            output.state.accounts().get(&target).unwrap().current.as_ref().unwrap().balance,
            U256::from(1)
        );
    }

    #[test]
    fn applies_withdrawals_to_evm2_block_output() {
        let existing = address!("0000000000000000000000000000000000000001");
        let new = address!("0000000000000000000000000000000000000002");
        let mut database = TestDatabase::default();
        database.accounts.insert(existing, AccountInfo::default().with_balance(U256::from(100)));
        let withdrawals = [
            Withdrawal { index: 0, validator_index: 0, address: existing, amount: 1 },
            Withdrawal { index: 1, validator_index: 1, address: new, amount: 2 },
            Withdrawal { index: 2, validator_index: 2, address: new, amount: 3 },
        ];

        let output = execute_evm2_block_with_withdrawals(
            SpecId::SHANGHAI,
            BlockEnv::default(),
            database,
            1,
            core::iter::empty::<Recovered<TransactionSigned>>(),
            Some(&withdrawals),
        )
        .expect("evm2 execution succeeds");

        assert!(output.result.receipts.is_empty());
        assert_eq!(
            output.state.accounts().get(&existing).unwrap().current.as_ref().unwrap().balance,
            U256::from(1_000_000_100)
        );
        assert_eq!(
            output.state.accounts().get(&new).unwrap().current.as_ref().unwrap().balance,
            U256::from(5_000_000_000u64)
        );
        assert_eq!(output.state.block_reverts().len(), 1);
    }

    #[test]
    fn rejects_missing_cancun_parent_beacon_root() {
        let err = execute_evm2_block_with_context(
            SpecId::CANCUN,
            BlockEnv::default(),
            TestDatabase::default(),
            1,
            core::iter::empty::<Recovered<TransactionSigned>>(),
            Evm2BlockExecutionContext {
                system_calls: Some(Evm2BlockSystemCalls {
                    parent_hash: B256::ZERO,
                    parent_beacon_block_root: None,
                }),
                withdrawals: None,
            },
        )
        .expect_err("missing parent beacon block root should fail");

        assert!(matches!(err, Evm2ExecutionError::MissingParentBeaconBlockRoot));
    }

    #[test]
    fn rejects_nonzero_cancun_genesis_parent_beacon_root() {
        let root = B256::from([1u8; 32]);
        let err = execute_evm2_block_with_context(
            SpecId::CANCUN,
            BlockEnv::default(),
            TestDatabase::default(),
            0,
            core::iter::empty::<Recovered<TransactionSigned>>(),
            Evm2BlockExecutionContext {
                system_calls: Some(Evm2BlockSystemCalls {
                    parent_hash: B256::ZERO,
                    parent_beacon_block_root: Some(root),
                }),
                withdrawals: None,
            },
        )
        .expect_err("nonzero Cancun genesis parent beacon block root should fail");

        assert!(matches!(
            err,
            Evm2ExecutionError::CancunGenesisParentBeaconBlockRootNotZero(actual) if actual == root
        ));
    }

    #[test]
    fn runs_pre_execution_system_calls_without_receipts() {
        let output = execute_evm2_block_with_context(
            SpecId::PRAGUE,
            BlockEnv::default(),
            TestDatabase::default(),
            1,
            core::iter::empty::<Recovered<TransactionSigned>>(),
            Evm2BlockExecutionContext {
                system_calls: Some(Evm2BlockSystemCalls {
                    parent_hash: B256::from([2u8; 32]),
                    parent_beacon_block_root: Some(B256::ZERO),
                }),
                withdrawals: None,
            },
        )
        .expect("system calls to absent contracts are no-ops");

        assert!(output.result.receipts.is_empty());
        assert!(output.state.accounts().is_empty());
        assert_eq!(output.state.block_reverts().len(), 1);
    }
}
