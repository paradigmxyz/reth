//! Scroll storage implementation.

#![cfg_attr(all(not(test), feature = "scroll"), warn(unused_crate_dependencies))]
// The `scroll` feature must be enabled to use this crate.
#![cfg(feature = "scroll")]

use alloy_primitives::{Address, B256, U256};
use reth_revm::{database::EvmStateProvider, primitives::Bytecode, Database, DatabaseRef};
use reth_scroll_primitives::{AccountExtension, ScrollPostExecutionContext};
use reth_scroll_revm::shared::AccountInfo;
use reth_storage_errors::provider::ProviderError;
use std::ops::{Deref, DerefMut};

/// A similar construct as `StateProviderDatabase` which captures additional Scroll context for
/// touched accounts during execution.
#[derive(Clone, Debug)]
pub struct ScrollStateProviderDatabase<DB> {
    /// Scroll post execution context.
    pub post_execution_context: ScrollPostExecutionContext,
    /// The database.
    pub db: DB,
}

impl<DB> Deref for ScrollStateProviderDatabase<DB> {
    type Target = DB;

    fn deref(&self) -> &Self::Target {
        &self.db
    }
}

impl<DB> DerefMut for ScrollStateProviderDatabase<DB> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.db
    }
}

impl<DB> ScrollStateProviderDatabase<DB> {
    /// Creates a [`ScrollStateProviderDatabase`] from the provided DB.
    pub fn new(db: DB) -> Self {
        Self { db, post_execution_context: Default::default() }
    }

    /// Consume [`ScrollStateProviderDatabase`] and return inner [`EvmStateProvider`].
    pub fn into_inner(self) -> DB {
        self.db
    }
}

impl<DB: EvmStateProvider> Database for ScrollStateProviderDatabase<DB> {
    type Error = ProviderError;

    /// Retrieves basic account information for a given address. Caches the Scroll account extension
    /// for the touched account if it has bytecode.
    fn basic(&mut self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        let Some(account) = self.db.basic_account(address)? else { return Ok(None) };
        let Some(code_hash) = account.bytecode_hash else { return Ok(Some(account.into())) };

        if let Some(AccountExtension { code_size, poseidon_code_hash: Some(hash) }) =
            account.account_extension
        {
            self.post_execution_context.entry(code_hash).or_insert((code_size, hash));
        }

        Ok(Some(account.into()))
    }

    fn code_by_hash(&mut self, code_hash: B256) -> Result<Bytecode, Self::Error> {
        Ok(self.db.bytecode_by_hash(code_hash)?.unwrap_or_default().0)
    }

    fn storage(&mut self, address: Address, index: U256) -> Result<U256, Self::Error> {
        Ok(self.db.storage(address, B256::new(index.to_be_bytes()))?.unwrap_or_default())
    }

    fn block_hash(&mut self, number: u64) -> Result<B256, Self::Error> {
        Ok(self.db.block_hash(number)?.unwrap_or_default())
    }
}

impl<DB: EvmStateProvider> DatabaseRef for ScrollStateProviderDatabase<DB> {
    type Error = ProviderError;

    fn basic_ref(&self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        Ok(self.db.basic_account(address)?.map(Into::into))
    }

    fn code_by_hash_ref(&self, code_hash: B256) -> Result<Bytecode, Self::Error> {
        Ok(self.db.bytecode_by_hash(code_hash)?.unwrap_or_default().0)
    }

    fn storage_ref(&self, address: Address, index: U256) -> Result<U256, Self::Error> {
        Ok(self.db.storage(address, B256::new(index.to_be_bytes()))?.unwrap_or_default())
    }

    fn block_hash_ref(&self, number: u64) -> Result<B256, Self::Error> {
        Ok(self.db.block_hash(number)?.unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use crate::ScrollStateProviderDatabase;
    use alloy_primitives::{keccak256, Address, Bytes, U256};
    use reth_codecs::{test_utils::UnusedBits, validate_bitflag_backwards_compat};
    use reth_primitives_traits::Account;
    use reth_revm::{test_utils::StateProviderTest, Database};
    use reth_scroll_primitives::{hash_code, AccountExtension};

    #[test]
    fn test_ensure_account_backwards_compatibility() {
        // See `reth-codecs/src/test-utils` for details.
        assert_eq!(Account::bitflag_encoded_bytes(), 2);
        assert_eq!(AccountExtension::bitflag_encoded_bytes(), 1);

        // In case of failure, refer to the documentation of the
        // [`validate_bitflag_backwards_compat`] macro for detailed instructions on handling
        // it.
        validate_bitflag_backwards_compat!(Account, UnusedBits::NotZero);
        validate_bitflag_backwards_compat!(AccountExtension, UnusedBits::NotZero);
    }

    #[test]
    fn test_scroll_post_execution_context() -> eyre::Result<()> {
        let mut db = StateProviderTest::default();

        // insert an eoa in the db
        let eoa_address = Address::random();
        let eoa = Account {
            nonce: 0,
            balance: U256::MAX,
            bytecode_hash: None,
            account_extension: Some(AccountExtension::empty()),
        };
        db.insert_account(eoa_address, eoa, None, Default::default());

        // insert a contract account in the db
        let contract_address = Address::random();
        let bytecode = Bytes::copy_from_slice(&[0x0, 0x1, 0x2, 0x3, 0x4, 0x5]);
        let bytecode_hash = keccak256(&bytecode);
        let contract = Account {
            nonce: 0,
            balance: U256::MAX,
            bytecode_hash: Some(bytecode_hash),
            account_extension: Some(AccountExtension::from_bytecode(&bytecode)),
        };
        db.insert_account(contract_address, contract, Some(bytecode.clone()), Default::default());

        // insert an empty contract account in the db
        let empty_contract_address = Address::random();
        let empty_bytecode = Bytes::copy_from_slice(&[]);
        let empty_contract = Account {
            nonce: 0,
            balance: U256::MAX,
            bytecode_hash: None,
            account_extension: Some(AccountExtension::empty()),
        };
        db.insert_account(
            empty_contract_address,
            empty_contract,
            Some(empty_bytecode),
            Default::default(),
        );

        let mut provider = ScrollStateProviderDatabase::new(db);

        // check eoa is in db
        let _ = provider.basic(eoa_address)?.unwrap();
        // check contract is in db
        let _ = provider.basic(contract_address)?.unwrap();
        // check empty contract is in db
        let _ = provider.basic(empty_contract_address)?.unwrap();

        // check provider context only contains contract
        let post_execution_context = provider.post_execution_context;
        assert_eq!(post_execution_context.len(), 1);

        // check post execution context is correct for contract
        let (code_size, poseidon_code_hash) = post_execution_context.get(&bytecode_hash).unwrap();
        assert_eq!(*code_size, 6);
        assert_eq!(*poseidon_code_hash, hash_code(&bytecode));

        Ok(())
    }
}
