//! Evm2 database adapters.

use crate::database::{EvmStateProvider, StateProviderDatabase};
use alloy_primitives::{Address, B256, U256};
use evm2::{
    bytecode::Bytecode,
    evm::{AccountInfo, Database},
    interpreter::Word,
};
use reth_primitives_traits::Account;
use reth_storage_errors::provider::ProviderError;

impl<DB> Database for StateProviderDatabase<DB>
where
    DB: EvmStateProvider + Send + 'static,
{
    type Error = ProviderError;

    fn get_account(&mut self, address: &Address) -> Result<Option<AccountInfo>, Self::Error> {
        Ok(self.basic_account(address)?.map(account_to_evm2))
    }

    fn get_code_by_hash(&mut self, code_hash: &B256) -> Result<Bytecode, Self::Error> {
        Ok(self
            .bytecode_by_hash(code_hash)?
            .map(|bytecode| Bytecode::new_raw(bytecode.original_bytes()))
            .unwrap_or_default())
    }

    fn get_storage(&mut self, address: &Address, key: &Word) -> Result<Word, Self::Error> {
        Ok(self.storage(*address, B256::new(key.to_be_bytes()))?.unwrap_or_default())
    }

    fn get_block_hash(&mut self, number: &Word) -> Result<Option<B256>, Self::Error> {
        let number = u256_to_u64_saturating(*number);
        self.block_hash(number)
    }
}

fn account_to_evm2(account: Account) -> AccountInfo {
    AccountInfo {
        balance: account.balance,
        nonce: account.nonce,
        code_hash: account.get_bytecode_hash(),
        code: None,
        _non_exhaustive: (),
    }
}

fn u256_to_u64_saturating(value: U256) -> u64 {
    if value > U256::from(u64::MAX) {
        u64::MAX
    } else {
        value.to()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::StateProviderTest;
    use alloy_primitives::{address, b256, map::HashMap, Bytes, U256};

    #[test]
    fn evm2_database_reads_account_code_storage_and_block_hash() {
        let address = address!("0000000000000000000000000000000000000001");
        let storage_key = B256::new(U256::from(1).to_be_bytes());
        let storage_value = U256::from(2);
        let code = Bytes::from_static(&[0x60, 0x00, 0x00]);
        let code_hash = alloy_primitives::keccak256(&code);
        let block_hash =
            b256!("0x1111111111111111111111111111111111111111111111111111111111111111");

        let mut provider = StateProviderTest::default();
        let mut storage = HashMap::default();
        storage.insert(storage_key, storage_value);
        provider.insert_account(
            address,
            Account { nonce: 7, balance: U256::from(100), bytecode_hash: Some(code_hash) },
            Some(code.clone()),
            storage,
        );
        provider.insert_block_hash(5, block_hash);

        let mut db = StateProviderDatabase::new(provider);

        let account = db.get_account(&address).unwrap().unwrap();
        assert_eq!(account.nonce, 7);
        assert_eq!(account.balance, U256::from(100));
        assert_eq!(account.code_hash, code_hash);
        assert!(account.code.is_none());

        let loaded_code = db.get_code_by_hash(&code_hash).unwrap();
        assert_eq!(loaded_code.original_bytes(), code);

        assert_eq!(db.get_storage(&address, &U256::from(1)).unwrap(), storage_value);
        assert_eq!(db.get_block_hash(&U256::from(5)).unwrap(), Some(block_hash));
    }
}
