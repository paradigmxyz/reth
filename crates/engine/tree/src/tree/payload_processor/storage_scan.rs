//! Loads a contract's storage into the page cache in parallel range scans.
//!
//! A transaction that walks a chain of dependent storage reads through one contract (a linked
//! list, a sorted set) reads one key per round trip to the disk on a cold page cache, and no probe
//! can load the next key before the previous value is known. Scanning the contract's storage in
//! parallel turns the rest of the walk into reads from memory.

use alloy_primitives::{keccak256, Address, B256};
use reth_db_api::{cursor::DbDupCursorRO, tables, transaction::DbTx};
use reth_provider::{DBProvider, DatabaseProviderFactory, ProviderResult, StorageSettingsCache};
use tracing::trace;

/// Number of ranges of the hashed slot space scanned in parallel, each on its own thread and read
/// transaction.
const SCAN_RANGES: usize = 64;

/// Upper bound on the slots one range scans, so that a contract with a very large storage costs a
/// bounded amount of reads (64 x 32,768 = 2M slots, ~100 MB).
const MAX_RANGE_SLOTS: usize = 32_768;

/// Reads the storage of `address` into the page cache and returns how many slots it read.
///
/// Splits the slot space into [`SCAN_RANGES`] ranges walked in parallel with duplicate cursors
/// over the table execution reads storage from, stopping each range after [`MAX_RANGE_SLOTS`]
/// slots. Errors end the affected range early: this only warms the page cache.
pub(crate) fn load_storage<F>(factory: &F, address: Address) -> usize
where
    F: DatabaseProviderFactory<Provider: StorageSettingsCache> + Sync,
{
    let slots = std::thread::scope(|scope| {
        let ranges = (0..SCAN_RANGES)
            .map(|range| {
                std::thread::Builder::new()
                    .name(format!("storage-scan-{range:02}"))
                    .spawn_scoped(scope, move || scan_range(factory, address, range).unwrap_or(0))
            })
            .collect::<Vec<_>>();
        ranges
            .into_iter()
            .filter_map(Result::ok)
            .map(|handle| handle.join().unwrap_or_default())
            .sum()
    });
    trace!(target: "engine::tree::payload_processor::prewarm", %address, slots, "Loaded storage");
    slots
}

/// Walks the slots of `address` in the `range`-th of [`SCAN_RANGES`] ranges of the slot space.
fn scan_range<F>(factory: &F, address: Address, range: usize) -> ProviderResult<usize>
where
    F: DatabaseProviderFactory<Provider: StorageSettingsCache>,
{
    let (start, end) =
        (range_start(range), (range + 1 < SCAN_RANGES).then(|| range_start(range + 1)));
    let provider = factory.database_provider_ro()?;
    let tx = provider.tx_ref();
    let mut slots = 0;
    if provider.cached_storage_settings().use_hashed_state() {
        let mut cursor = tx.cursor_dup_read::<tables::HashedStorages>()?;
        let mut entry = cursor.seek_by_key_subkey(keccak256(address), start)?;
        while let Some(slot) = entry &&
            end.is_none_or(|end| slot.key < end) &&
            slots < MAX_RANGE_SLOTS
        {
            slots += 1;
            entry = cursor.next_dup_val()?;
        }
    } else {
        let mut cursor = tx.cursor_dup_read::<tables::PlainStorageState>()?;
        let mut entry = cursor.seek_by_key_subkey(address, start)?;
        while let Some(slot) = entry &&
            end.is_none_or(|end| slot.key < end) &&
            slots < MAX_RANGE_SLOTS
        {
            slots += 1;
            entry = cursor.next_dup_val()?;
        }
    }
    Ok(slots)
}

/// Returns the first slot of the `range`-th of [`SCAN_RANGES`] equal ranges of the slot space.
fn range_start(range: usize) -> B256 {
    let mut start = B256::ZERO;
    let prefix = (range * (usize::from(u16::MAX) + 1) / SCAN_RANGES) as u16;
    start.0[..2].copy_from_slice(&prefix.to_be_bytes());
    start
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::U256;
    use reth_db::transaction::DbTxMut;
    use reth_primitives_traits::StorageEntry;
    use reth_provider::test_utils::create_test_provider_factory;

    #[test]
    fn load_storage_reads_every_slot_of_the_contract() {
        let factory = create_test_provider_factory();
        let address = Address::with_last_byte(1);
        let other = Address::with_last_byte(2);
        let provider = factory.database_provider_rw().unwrap();
        let hashed = provider.cached_storage_settings().use_hashed_state();
        for (owner, count) in [(address, 3_000u64), (other, 100)] {
            for i in 0..count {
                let entry = StorageEntry {
                    key: keccak256(B256::from(U256::from(i))),
                    value: U256::from(1),
                };
                if hashed {
                    provider
                        .tx_ref()
                        .put::<tables::HashedStorages>(keccak256(owner), entry)
                        .unwrap();
                } else {
                    provider.tx_ref().put::<tables::PlainStorageState>(owner, entry).unwrap();
                }
            }
        }
        provider.commit().unwrap();

        assert_eq!(load_storage(&factory, address), 3_000);
        assert_eq!(load_storage(&factory, Address::with_last_byte(3)), 0);
    }

    #[test]
    fn ranges_cover_the_slot_space_in_order() {
        assert_eq!(range_start(0), B256::ZERO);
        let starts = (0..SCAN_RANGES).map(range_start).collect::<Vec<_>>();
        assert!(starts.windows(2).all(|pair| pair[0] < pair[1]));
    }
}
