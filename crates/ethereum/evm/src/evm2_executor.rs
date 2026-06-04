//! Evm2-backed Ethereum execution helpers.

use crate::{evm2_recovered_tx, RethReceiptBuilder};
use alloc::vec::Vec;
use alloy_consensus::transaction::Recovered;
use evm2::{
    env::BlockEnv,
    ethereum::ethereum_tx_registry,
    evm::{Database, Db, DbErrorCode},
    registry::HandlerError,
    BaseEvmTypes, Evm, Precompiles, SpecId,
};
use reth_ethereum_primitives::{Receipt, TransactionSigned};
use reth_execution_types::BlockExecutionOutput;

/// Error returned by evm2-backed Ethereum execution.
#[derive(Debug)]
pub enum Evm2ExecutionError<E> {
    /// Evm2 rejected or halted transaction execution before producing a Reth output.
    Handler(HandlerError),
    /// Evm2 reported a database error and the typed database error was available.
    Database(E),
    /// Evm2 reported a database error, but the typed database error was no longer available.
    MissingDatabaseError(DbErrorCode),
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
        }
    }
}

impl<E> core::error::Error for Evm2ExecutionError<E> where E: core::error::Error + Send + 'static {}

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
    let mut evm = Evm::<BaseEvmTypes>::new(
        spec_id,
        block_env,
        ethereum_tx_registry(spec_id),
        Db::new(database),
        Precompiles::base(spec_id),
    );
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

    Ok(RethReceiptBuilder.build_evm2_block_output(block_number, results))
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

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::collections::BTreeMap;
    use alloy_consensus::{SignableTransaction, TxLegacy};
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
}
