use alloy_primitives::{Address, B256};
use reth_primitives_traits::{Account, Bytecode};
use reth_storage_api::{errors::provider::ProviderResult, AccountReader, BytecodeReader};

/// Stand-in state provider for test/mock validators that don't have a database.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct NoopAccountInfoReader;

impl AccountReader for NoopAccountInfoReader {
    fn basic_account(&self, _address: &Address) -> ProviderResult<Option<Account>> {
        Ok(Some(Account::default()))
    }
}

impl BytecodeReader for NoopAccountInfoReader {
    fn bytecode_by_hash(&self, _code_hash: &B256) -> ProviderResult<Option<Bytecode>> {
        Ok(None)
    }
}

/// Used by mock validators to satisfy [`TransactionValidator::validate_stateful`] without calling
/// `latest_state()`.
pub(crate) static NOOP_ACCOUNT_READER: NoopAccountInfoReader = NoopAccountInfoReader;
