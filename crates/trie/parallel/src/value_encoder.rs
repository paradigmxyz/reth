use alloy_primitives::B256;
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

/// Returned from [`AsyncAccountValueEncoder`] to calculate a storage root inline when encoded.
pub(crate) enum AsyncAccountDeferredValueEncoder<TC, HC> {
    /// Synchronous storage root computation.
    Sync {
        /// Shared storage proof calculator for computing storage roots.
        storage_calculator: Rc<RefCell<StorageProofCalculator<TC, HC>>>,
        hashed_address: B256,
        account: Account,
    },
}

impl<TC, HC> DeferredValueEncoder for AsyncAccountDeferredValueEncoder<TC, HC>
where
    TC: TrieStorageCursor,
    HC: HashedStorageCursor<Value = alloy_primitives::U256>,
{
    fn encode(self, buf: &mut Vec<u8>) -> Result<(), StateProofError> {
        let Self::Sync { storage_calculator, hashed_address, account } = self;
        let mut calculator = storage_calculator.borrow_mut();
        let root_node = calculator.storage_root_node(hashed_address)?;
        let storage_root = calculator
            .compute_root_hash(&[root_node])?
            .expect("storage_root_node returns a node at empty path");
        account.into_trie_account(storage_root).encode(buf);
        Ok(())
    }
}

/// Implements the [`LeafValueEncoder`] trait for accounts.
///
/// Uses a shared [`StorageProofCalculator`] to compute storage roots synchronously, reusing
/// cursors across multiple accounts.
pub(crate) struct AsyncAccountValueEncoder<TC, HC> {
    /// Shared storage proof calculator for synchronous computation. Reuses cursors and internal
    /// buffers across multiple storage root calculations.
    storage_calculator: Rc<RefCell<StorageProofCalculator<TC, HC>>>,
}

impl<TC, HC> AsyncAccountValueEncoder<TC, HC> {
    /// Initializes a [`Self`] using a storage proof calculator which will be reused to calculate
    /// storage roots synchronously.
    pub(crate) const fn new(
        storage_calculator: Rc<RefCell<StorageProofCalculator<TC, HC>>>,
    ) -> Self {
        Self { storage_calculator }
    }
}

impl<TC, HC> LeafValueEncoder for AsyncAccountValueEncoder<TC, HC>
where
    TC: TrieStorageCursor,
    HC: HashedStorageCursor<Value = alloy_primitives::U256>,
{
    type Value = Account;
    type DeferredEncoder = AsyncAccountDeferredValueEncoder<TC, HC>;

    fn deferred_encoder(
        &mut self,
        hashed_address: B256,
        account: Self::Value,
    ) -> Self::DeferredEncoder {
        AsyncAccountDeferredValueEncoder::Sync {
            storage_calculator: self.storage_calculator.clone(),
            hashed_address,
            account,
        }
    }
}
