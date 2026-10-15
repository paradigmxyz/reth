use alloy_primitives::{B256, U256};
use alloy_rlp::Encodable;
use core::cell::RefCell;
use reth_execution_errors::trie::StateProofError;
use reth_primitives_traits::Account;
use reth_trie::{
    hashed_cursor::HashedStorageCursor,
    proof_v2::{DeferredValueEncoder, LeafValueEncoder, StorageProofCalculator},
    trie_cursor::TrieStorageCursor,
};
use std::rc::Rc;

/// Implements the [`LeafValueEncoder`] trait for accounts.
///
/// Storage roots are calculated with a shared [`StorageProofCalculator`], reusing its cursors and
/// buffers across accounts.
pub(crate) struct AccountValueEncoder<TC, HC> {
    /// Calculator used for the storage root of every encoded account.
    storage_calculator: Rc<RefCell<StorageProofCalculator<TC, HC>>>,
}

impl<TC, HC> AccountValueEncoder<TC, HC> {
    /// Creates an encoder that calculates storage roots with `storage_calculator`.
    pub(crate) const fn new(
        storage_calculator: Rc<RefCell<StorageProofCalculator<TC, HC>>>,
    ) -> Self {
        Self { storage_calculator }
    }
}

impl<TC, HC> LeafValueEncoder for AccountValueEncoder<TC, HC>
where
    TC: TrieStorageCursor,
    HC: HashedStorageCursor<Value = U256>,
{
    type Value = Account;
    type DeferredEncoder = AccountDeferredValueEncoder<TC, HC>;

    fn deferred_encoder(
        &mut self,
        hashed_address: B256,
        account: Self::Value,
    ) -> Self::DeferredEncoder {
        AccountDeferredValueEncoder {
            storage_calculator: self.storage_calculator.clone(),
            hashed_address,
            account,
        }
    }
}

/// Returned from [`AccountValueEncoder`] to calculate an account's storage root when encoded.
pub(crate) struct AccountDeferredValueEncoder<TC, HC> {
    /// Calculator shared with the [`AccountValueEncoder`] that created this encoder.
    storage_calculator: Rc<RefCell<StorageProofCalculator<TC, HC>>>,
    hashed_address: B256,
    account: Account,
}

impl<TC, HC> DeferredValueEncoder for AccountDeferredValueEncoder<TC, HC>
where
    TC: TrieStorageCursor,
    HC: HashedStorageCursor<Value = U256>,
{
    fn encode(self, buf: &mut Vec<u8>) -> Result<(), StateProofError> {
        let mut calculator = self.storage_calculator.borrow_mut();
        let root_node = calculator.storage_root_node(self.hashed_address)?;
        let storage_root = calculator
            .compute_root_hash(&[root_node])?
            .expect("storage_root_node returns a node at empty path");
        self.account.into_trie_account(storage_root).encode(buf);
        Ok(())
    }
}
