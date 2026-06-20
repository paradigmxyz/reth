use alloy_primitives::B256;
use reth_storage_api::{StorageRangeEntry, StorageRangeResult};
use reth_storage_errors::provider::ProviderResult;
use reth_trie::hashed_cursor::{HashedCursor, HashedCursorFactory};

/// Collects paginated storage entries from a [`HashedCursorFactory`] for a given account.
///
/// Seeks to `start` on the hashed storage cursor, collects up to `limit` entries,
/// and captures the next key if more entries exist.
pub(super) fn storage_range<H: HashedCursorFactory>(
    hashed_cursor_factory: &H,
    hashed_address: B256,
    start: B256,
    limit: usize,
) -> ProviderResult<StorageRangeResult> {
    let mut cursor = hashed_cursor_factory.hashed_storage_cursor(hashed_address)?;

    let mut entries = Vec::new();
    let mut entry = cursor.seek(start)?;

    while let Some((hash, value)) = entry {
        if entries.len() >= limit {
            return Ok(StorageRangeResult { entries, next_key: Some(hash) });
        }
        entries.push(StorageRangeEntry { hash, value });
        entry = cursor.next()?;
    }

    Ok(StorageRangeResult { entries, next_key: None })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{b256, map::B256Map, U256};
    use reth_trie::hashed_cursor::mock::MockHashedCursorFactory;
    use std::collections::BTreeMap;

    fn make_factory(hashed_address: B256, slots: Vec<(B256, U256)>) -> MockHashedCursorFactory {
        let storage: BTreeMap<B256, U256> = slots.into_iter().collect();
        let mut storage_tries: B256Map<BTreeMap<B256, U256>> = B256Map::default();
        storage_tries.insert(hashed_address, storage);
        MockHashedCursorFactory::new(BTreeMap::new(), storage_tries)
    }

    const ADDR: B256 = b256!("0x1111111111111111111111111111111111111111111111111111111111111111");

    fn slot(n: u8) -> B256 {
        let mut bytes = [0u8; 32];
        bytes[31] = n;
        B256::from(bytes)
    }

    #[test]
    fn basic_pagination() {
        let slots = vec![
            (slot(1), U256::from(10)),
            (slot(2), U256::from(20)),
            (slot(3), U256::from(30)),
            (slot(4), U256::from(40)),
            (slot(5), U256::from(50)),
        ];
        let factory = make_factory(ADDR, slots);

        let result = storage_range(&factory, ADDR, B256::ZERO, 3).unwrap();

        assert_eq!(result.entries.len(), 3);
        assert_eq!(result.entries[0].hash, slot(1));
        assert_eq!(result.entries[0].value, U256::from(10));
        assert_eq!(result.entries[1].hash, slot(2));
        assert_eq!(result.entries[2].hash, slot(3));
        assert_eq!(result.next_key, Some(slot(4)));
    }

    #[test]
    fn start_key_seeking() {
        let slots =
            vec![(slot(1), U256::from(10)), (slot(3), U256::from(30)), (slot(5), U256::from(50))];
        let factory = make_factory(ADDR, slots);

        let result = storage_range(&factory, ADDR, slot(2), 10).unwrap();

        assert_eq!(result.entries.len(), 2);
        assert_eq!(result.entries[0].hash, slot(3));
        assert_eq!(result.entries[1].hash, slot(5));
        assert_eq!(result.next_key, None);
    }

    #[test]
    fn exact_start_key_inclusive() {
        let slots =
            vec![(slot(1), U256::from(10)), (slot(2), U256::from(20)), (slot(3), U256::from(30))];
        let factory = make_factory(ADDR, slots);

        let result = storage_range(&factory, ADDR, slot(2), 10).unwrap();

        assert_eq!(result.entries.len(), 2);
        assert_eq!(result.entries[0].hash, slot(2));
        assert_eq!(result.entries[0].value, U256::from(20));
        assert_eq!(result.entries[1].hash, slot(3));
        assert_eq!(result.next_key, None);
    }

    #[test]
    fn empty_storage() {
        let factory = make_factory(ADDR, vec![]);

        let result = storage_range(&factory, ADDR, B256::ZERO, 10).unwrap();

        assert!(result.entries.is_empty());
        assert_eq!(result.next_key, None);
    }

    #[test]
    fn limit_zero() {
        let slots = vec![(slot(1), U256::from(10)), (slot(2), U256::from(20))];
        let factory = make_factory(ADDR, slots);

        let result = storage_range(&factory, ADDR, B256::ZERO, 0).unwrap();

        assert!(result.entries.is_empty());
        assert_eq!(result.next_key, Some(slot(1)));
    }

    #[test]
    fn limit_exceeds_slot_count() {
        let slots = vec![(slot(1), U256::from(10)), (slot(2), U256::from(20))];
        let factory = make_factory(ADDR, slots);

        let result = storage_range(&factory, ADDR, B256::ZERO, 100).unwrap();

        assert_eq!(result.entries.len(), 2);
        assert_eq!(result.entries[0].hash, slot(1));
        assert_eq!(result.entries[1].hash, slot(2));
        assert_eq!(result.next_key, None);
    }

    #[test]
    fn single_slot() {
        let slots = vec![(slot(5), U256::from(42))];
        let factory = make_factory(ADDR, slots);

        let result = storage_range(&factory, ADDR, B256::ZERO, 10).unwrap();

        assert_eq!(result.entries.len(), 1);
        assert_eq!(result.entries[0].hash, slot(5));
        assert_eq!(result.entries[0].value, U256::from(42));
        assert_eq!(result.next_key, None);
    }
}
