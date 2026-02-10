use alloc::{
    collections::{BTreeMap, BTreeSet},
    vec::Vec,
};
use alloy_primitives::{Address, BlockNumber, B256};
use core::ops::{RangeBounds, RangeInclusive};
use reth_primitives_traits::StorageEntry;
use reth_storage_errors::provider::ProviderResult;

/// Storage reader
#[auto_impl::auto_impl(&, Box)]
pub trait StorageReader: Send {
    /// Get plainstate storages for addresses and storage keys.
    fn plain_state_storages(
        &self,
        addresses_with_keys: impl IntoIterator<Item = (Address, impl IntoIterator<Item = B256>)>,
    ) -> ProviderResult<Vec<(Address, Vec<StorageEntry>)>>;

    /// Iterate over storage changesets and return all storage slots that were changed.
    fn changed_storages_with_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<BTreeMap<Address, BTreeSet<B256>>>;

    /// Iterate over storage changesets and return all storage slots that were changed alongside
    /// each specific set of blocks.
    ///
    /// NOTE: Get inclusive range of blocks.
    fn changed_storages_and_blocks_with_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<BTreeMap<(Address, B256), Vec<u64>>>;
}

/// Storage `ChangeSet` reader
#[cfg(feature = "db-api")]
#[auto_impl::auto_impl(&, Box)]
pub trait StorageChangeSetReader: Send {
    /// Iterate over storage changesets and return the storage state from before this block.
    fn storage_changeset(
        &self,
        block_number: BlockNumber,
    ) -> ProviderResult<Vec<(reth_db_api::models::BlockNumberAddress, StorageEntry)>>;

    /// Search the block's changesets for the given address and storage key, and return the result.
    ///
    /// Returns `None` if the storage slot was not changed in this block.
    fn get_storage_before_block(
        &self,
        block_number: BlockNumber,
        address: Address,
        storage_key: B256,
    ) -> ProviderResult<Option<StorageEntry>>;

    /// Get all storage changesets in a range of blocks.
    fn storage_changesets_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<(reth_db_api::models::BlockNumberAddress, StorageEntry)>>;

    /// Get the total count of all storage changes.
    fn storage_changeset_count(&self) -> ProviderResult<usize>;

    /// Get storage changesets for a block as static-file rows.
    ///
    /// Default implementation uses `storage_changeset` and maps to `StorageBeforeTx`.
    fn storage_block_changeset(
        &self,
        block_number: BlockNumber,
    ) -> ProviderResult<Vec<reth_db_models::StorageBeforeTx>> {
        self.storage_changeset(block_number).map(|changesets| {
            changesets
                .into_iter()
                .map(|(block_address, entry)| reth_db_models::StorageBeforeTx {
                    address: block_address.address(),
                    key: entry.key,
                    value: entry.value,
                })
                .collect()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::B256;

    #[test]
    fn changeset_entry_into_storage_entry_preserves_value() {
        let entry = ChangesetEntry {
            key: StorageSlotKey::Plain(B256::repeat_byte(0x11)),
            value: U256::from(42),
        };
        let storage: StorageEntry = entry.into_storage_entry();
        assert_eq!(storage.key, entry.key.as_b256());
        assert_eq!(storage.value, entry.value);
    }

    #[test]
    fn changeset_entry_from_preserves_value() {
        let entry = ChangesetEntry {
            key: StorageSlotKey::Hashed(B256::repeat_byte(0x22)),
            value: U256::from(100),
        };
        let storage: StorageEntry = entry.into();
        assert_eq!(storage.key, entry.key.as_b256());
        assert_eq!(storage.value, entry.value);
    }

    #[test]
    fn changeset_entry_default_is_plain_zero() {
        let entry = ChangesetEntry::default();
        assert!(entry.key.is_plain());
        assert_eq!(entry.key.as_b256(), B256::ZERO);
        assert_eq!(entry.value, U256::ZERO);
    }

    #[test]
    fn changeset_entry_ord_by_key_then_value() {
        let lo_key = ChangesetEntry {
            key: StorageSlotKey::Plain(B256::repeat_byte(0x01)),
            value: U256::from(999),
        };
        let hi_key = ChangesetEntry {
            key: StorageSlotKey::Plain(B256::repeat_byte(0x02)),
            value: U256::from(1),
        };
        assert!(lo_key < hi_key, "ordering should be by key first");

        let same_key_lo_val = ChangesetEntry {
            key: StorageSlotKey::Plain(B256::repeat_byte(0x01)),
            value: U256::from(1),
        };
        let same_key_hi_val = ChangesetEntry {
            key: StorageSlotKey::Plain(B256::repeat_byte(0x01)),
            value: U256::from(2),
        };
        assert!(same_key_lo_val < same_key_hi_val, "same key: order by value");
    }

    #[test]
    fn changeset_entry_ord_plain_before_hashed() {
        let plain = ChangesetEntry {
            key: StorageSlotKey::Plain(B256::repeat_byte(0xFF)),
            value: U256::MAX,
        };
        let hashed = ChangesetEntry {
            key: StorageSlotKey::Hashed(B256::ZERO),
            value: U256::ZERO,
        };
        assert!(plain < hashed, "Plain variant (discriminant 0) < Hashed (discriminant 1)");
    }

    #[test]
    fn changeset_entry_hashed_into_storage_entry_produces_correct_b256() {
        let h = B256::repeat_byte(0xAA);
        let entry = ChangesetEntry { key: StorageSlotKey::Hashed(h), value: U256::from(77) };
        let se = entry.into_storage_entry();
        assert_eq!(se.key, h, "as_b256 on Hashed should return the inner hash");
        assert_eq!(se.value, U256::from(77));
    }

    #[test]
    fn changeset_entry_from_and_into_storage_entry_are_identical() {
        for (key, value) in [
            (StorageSlotKey::Plain(B256::repeat_byte(0x11)), U256::from(1)),
            (StorageSlotKey::Hashed(B256::repeat_byte(0x22)), U256::from(2)),
            (StorageSlotKey::Plain(B256::ZERO), U256::ZERO),
            (StorageSlotKey::Hashed(B256::from([0xFF; 32])), U256::MAX),
        ] {
            let entry = ChangesetEntry { key, value };
            let via_method = entry.into_storage_entry();
            let via_from: StorageEntry = entry.into();
            assert_eq!(via_method.key, via_from.key);
            assert_eq!(via_method.value, via_from.value);
        }
    }

    #[test]
    #[cfg(not(debug_assertions))]
    fn to_changeset_key_hashed_release_returns_raw_hash() {
        let h = alloy_primitives::keccak256(B256::repeat_byte(0x66));
        let result = StorageSlotKey::Hashed(h).to_changeset_key(false);
        assert_eq!(result, h, "BUG-1: in release, Hashed.to_changeset_key(false) silently returns the raw hash");
    }

    #[test]
    #[cfg(not(debug_assertions))]
    fn to_changeset_key_hashed_true_release_accidentally_correct() {
        let h = alloy_primitives::keccak256(B256::repeat_byte(0x77));
        let result = StorageSlotKey::Hashed(h).to_changeset_key(true);
        assert_eq!(result, h, "Hashed.to_changeset_key(true) is accidentally correct: to_hashed is no-op on Hashed");
    }
}
