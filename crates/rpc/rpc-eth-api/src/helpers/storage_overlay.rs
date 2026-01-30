use crate::helpers::storage_diff_inspector::StorageDiffs;
use alloy_primitives::{Address, StorageKey, StorageValue};
use reth_errors::ProviderResult;
use reth_primitives_traits::StorageEntry;
use reth_storage_api::{StorageRangeProvider, StorageRangeResult};
use std::{
    collections::{BTreeMap, HashMap},
    fmt,
};

const STORAGE_PAGE: usize = 512;

/// Provides a storage-range view that overlays pending slot diffs on top of a historical provider.
pub struct StorageRangeOverlay<'a> {
    base: &'a dyn StorageRangeProvider,
    overlays: HashMap<Address, BTreeMap<StorageKey, StorageValue>>,
}

impl<'a> fmt::Debug for StorageRangeOverlay<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StorageRangeOverlay").field("overlays", &self.overlays).finish()
    }
}

impl<'a> StorageRangeOverlay<'a> {
    /// Creates a new overlay that delegates to `base` for accounts without pending diffs.
    pub fn new(base: &'a dyn StorageRangeProvider) -> Self {
        Self { base, overlays: HashMap::new() }
    }

    /// Registers the merged storage slots for `account` that should shadow the base provider.
    pub fn add_account_overlay(
        &mut self,
        account: Address,
        slots: BTreeMap<StorageKey, StorageValue>,
    ) {
        self.overlays.insert(account, slots);
    }

    /// Returns the merged slots for `account` taking into account diffs up to `tx_idx`
    /// (inclusive). If the account was untouched, `None` is returned.
    pub fn merged_slots_for_account(
        base: &dyn StorageRangeProvider,
        account: Address,
        diffs: &StorageDiffs,
        tx_idx: usize,
    ) -> ProviderResult<Option<BTreeMap<StorageKey, StorageValue>>> {
        if diffs.is_empty() {
            return Ok(None)
        }

        let limit = tx_idx.min(diffs.len().saturating_sub(1));
        let mut relevant: Vec<&BTreeMap<StorageKey, StorageValue>> = Vec::new();
        for tx_diff in diffs.iter().take(limit + 1) {
            if let Some(slots) = tx_diff.get(&account) {
                relevant.push(slots);
            }
        }

        if relevant.is_empty() {
            return Ok(None)
        }

        let mut merged = collect_account_slots(base, account)?;
        apply_diffs(&mut merged, relevant);
        Ok(Some(merged))
    }
}

impl StorageRangeProvider for StorageRangeOverlay<'_> {
    fn storage_range(
        &self,
        account: Address,
        start_key: StorageKey,
        max_slots: usize,
    ) -> ProviderResult<StorageRangeResult> {
        if let Some(overlay) = self.overlays.get(&account) {
            return Ok(range_from_map(overlay, start_key, max_slots))
        }

        self.base.storage_range(account, start_key, max_slots)
    }
}

fn range_from_map(
    map: &BTreeMap<StorageKey, StorageValue>,
    start_key: StorageKey,
    max_slots: usize,
) -> StorageRangeResult {
    if max_slots == 0 {
        return StorageRangeResult::empty()
    }

    let mut slots = Vec::new();
    let mut next_key = None;
    for (key, value) in map.range(start_key..) {
        if slots.len() < max_slots {
            slots.push(StorageEntry { key: *key, value: *value });
        } else {
            next_key = Some(*key);
            break;
        }
    }

    StorageRangeResult { slots, next_key }
}

fn collect_account_slots(
    provider: &dyn StorageRangeProvider,
    account: Address,
) -> ProviderResult<BTreeMap<StorageKey, StorageValue>> {
    let mut slots = BTreeMap::new();
    let mut start = StorageKey::ZERO;

    loop {
        let StorageRangeResult { slots: chunk_slots, next_key } =
            provider.storage_range(account, start, STORAGE_PAGE)?;
        for entry in &chunk_slots {
            slots.insert(entry.key, entry.value);
        }

        if let Some(next) = next_key {
            if chunk_slots.is_empty() && next == start {
                break
            }
            start = next;
        } else {
            break;
        }
    }

    Ok(slots)
}

fn apply_diffs<'a, I>(merged: &mut BTreeMap<StorageKey, StorageValue>, diffs: I)
where
    I: IntoIterator<Item = &'a BTreeMap<StorageKey, StorageValue>>,
{
    for diff in diffs {
        for (slot, value) in diff {
            if value.is_zero() {
                merged.remove(slot);
            } else {
                merged.insert(*slot, *value);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{StorageKey, StorageValue};

    struct TestRangeProvider {
        slots: BTreeMap<StorageKey, StorageValue>,
    }

    impl TestRangeProvider {
        fn new(slots: impl IntoIterator<Item = (u8, u64)>) -> Self {
            let mut map = BTreeMap::new();
            for (slot, value) in slots {
                map.insert(StorageKey::with_last_byte(slot), StorageValue::from(value));
            }
            Self { slots: map }
        }
    }

    impl StorageRangeProvider for TestRangeProvider {
        fn storage_range(
            &self,
            _account: Address,
            start_key: StorageKey,
            max_slots: usize,
        ) -> ProviderResult<StorageRangeResult> {
            Ok(range_from_map(&self.slots, start_key, max_slots))
        }
    }

    #[test]
    fn merges_diffs_and_removals() {
        let base = TestRangeProvider::new([(0x01, 10), (0x02, 20)]);
        let account = Address::repeat_byte(0x11);

        let mut tx_diff = HashMap::new();
        tx_diff.insert(
            account,
            BTreeMap::from([
                (StorageKey::with_last_byte(0x01), StorageValue::from(30)),
                (StorageKey::with_last_byte(0x02), StorageValue::ZERO),
                (StorageKey::with_last_byte(0x03), StorageValue::from(40)),
            ]),
        );
        let diffs = vec![tx_diff];

        let merged = StorageRangeOverlay::merged_slots_for_account(&base, account, &diffs, 0)
            .unwrap()
            .unwrap();

        assert_eq!(merged.len(), 2);
        assert_eq!(merged.get(&StorageKey::with_last_byte(0x01)), Some(&StorageValue::from(30)));
        assert_eq!(merged.get(&StorageKey::with_last_byte(0x03)), Some(&StorageValue::from(40)));
        assert!(!merged.contains_key(&StorageKey::with_last_byte(0x02)));
    }

    #[test]
    fn overlay_provider_prefers_overrides() {
        let base = TestRangeProvider::new([(0x01, 1), (0x02, 2)]);
        let account = Address::repeat_byte(0x44);

        let merged = BTreeMap::from([
            (StorageKey::with_last_byte(0x01), StorageValue::from(9)),
            (StorageKey::with_last_byte(0x04), StorageValue::from(4)),
        ]);

        let mut overlay = StorageRangeOverlay::new(&base);
        overlay.add_account_overlay(account, merged);

        let res = overlay.storage_range(account, StorageKey::ZERO, 10).expect("range");
        assert_eq!(res.slots.len(), 2);
        assert_eq!(res.slots[0].value, StorageValue::from(9));
        assert_eq!(res.slots[1].key, StorageKey::with_last_byte(0x04));

        // other accounts fall back to base provider
        let other = overlay.storage_range(Address::ZERO, StorageKey::ZERO, 10).expect("range");
        assert_eq!(other.slots.len(), 2);
        assert_eq!(other.slots[1].value, StorageValue::from(2));
    }
}
