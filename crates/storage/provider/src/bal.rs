use alloy_eip7928::BAL_RETENTION_PERIOD_SLOTS;
use alloy_primitives::{BlockHash, BlockNumber, Bytes};
use parking_lot::RwLock;
use reth_prune_types::PruneMode;
use reth_storage_api::{BalStore, GetBlockAccessListLimit};
use reth_storage_errors::provider::ProviderResult;
use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};

/// Basic in-memory BAL store keyed by block hash.
#[derive(Debug, Clone)]
pub struct InMemoryBalStore {
    config: BalConfig,
    inner: Arc<RwLock<InMemoryBalStoreInner>>,
}

impl InMemoryBalStore {
    /// Creates a new in-memory BAL store with the given config.
    pub fn new(config: BalConfig) -> Self {
        Self { config, inner: Arc::new(RwLock::new(InMemoryBalStoreInner::default())) }
    }
}

impl Default for InMemoryBalStore {
    fn default() -> Self {
        Self::new(BalConfig::default())
    }
}

/// Configuration for BAL storage.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct BalConfig {
    /// Retention policy for BALs kept in memory.
    in_memory_retention: Option<PruneMode>,
}

impl BalConfig {
    /// Returns a config with no in-memory BAL retention limit.
    pub const fn unbounded() -> Self {
        Self { in_memory_retention: None }
    }

    /// Returns a config with the given in-memory BAL retention policy.
    pub const fn with_in_memory_retention(in_memory_retention: PruneMode) -> Self {
        Self { in_memory_retention: Some(in_memory_retention) }
    }
}

impl Default for BalConfig {
    fn default() -> Self {
        Self::with_in_memory_retention(PruneMode::Distance(BAL_RETENTION_PERIOD_SLOTS))
    }
}

#[derive(Debug, Default)]
struct InMemoryBalStoreInner {
    entries: HashMap<BlockHash, BalEntry>,
    hashes_by_number: BTreeMap<BlockNumber, Vec<BlockHash>>,
    highest_block_number: Option<BlockNumber>,
}

impl InMemoryBalStoreInner {
    // Inserts a BAL and keeps the block-number index in sync.
    fn insert(&mut self, block_hash: BlockHash, block_number: BlockNumber, bal: Bytes) {
        let empty_block_number =
            self.entries.insert(block_hash, BalEntry { block_number, bal }).and_then(|entry| {
                let hashes = self.hashes_by_number.get_mut(&entry.block_number)?;
                hashes.retain(|hash| *hash != block_hash);
                hashes.is_empty().then_some(entry.block_number)
            });

        if let Some(block_number) = empty_block_number {
            self.hashes_by_number.remove(&block_number);
        }

        self.hashes_by_number.entry(block_number).or_default().push(block_hash);
        self.highest_block_number = Some(
            self.highest_block_number.map_or(block_number, |highest| highest.max(block_number)),
        );
    }

    // Removes BALs outside the configured retention window.
    fn prune(&mut self, prune_mode: Option<PruneMode>) {
        let Some(prune_mode) = prune_mode else { return };
        let Some(tip) = self.highest_block_number else { return };

        while let Some((&block_number, _)) = self.hashes_by_number.first_key_value() {
            if !prune_mode.should_prune(block_number, tip) {
                break
            }

            let Some((_, hashes)) = self.hashes_by_number.pop_first() else { break };
            for hash in hashes {
                self.entries.remove(&hash);
            }
        }
    }
}

#[derive(Debug)]
struct BalEntry {
    block_number: BlockNumber,
    bal: Bytes,
}

impl BalStore for InMemoryBalStore {
    fn insert(
        &self,
        block_hash: BlockHash,
        block_number: BlockNumber,
        bal: Bytes,
    ) -> ProviderResult<()> {
        let mut inner = self.inner.write();
        inner.insert(block_hash, block_number, bal);
        inner.prune(self.config.in_memory_retention);
        Ok(())
    }

    fn get_by_hashes(&self, block_hashes: &[BlockHash]) -> ProviderResult<Vec<Option<Bytes>>> {
        let inner = self.inner.read();
        let mut result = Vec::with_capacity(block_hashes.len());

        for hash in block_hashes {
            result.push(inner.entries.get(hash).map(|entry| entry.bal.clone()));
        }

        Ok(result)
    }

    fn append_by_hashes_with_limit(
        &self,
        block_hashes: &[BlockHash],
        limit: GetBlockAccessListLimit,
        out: &mut Vec<Bytes>,
    ) -> ProviderResult<()> {
        let inner = self.inner.read();
        let mut size = 0;

        for hash in block_hashes {
            let bal = inner
                .entries
                .get(hash)
                .map(|entry| entry.bal.clone())
                .unwrap_or_else(|| Bytes::from_static(&[0xc0]));
            size += bal.len();
            out.push(bal);

            if limit.exceeds(size) {
                break
            }
        }

        Ok(())
    }

    fn get_by_range(&self, _start: BlockNumber, _count: u64) -> ProviderResult<Vec<Bytes>> {
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::B256;

    #[test]
    fn insert_and_lookup_by_hash() {
        let store = InMemoryBalStore::default();
        let hash = B256::random();
        let missing = B256::random();
        let bal = Bytes::from_static(b"bal");

        store.insert(hash, 1, bal.clone()).unwrap();

        assert_eq!(store.get_by_hashes(&[hash, missing]).unwrap(), vec![Some(bal), None]);
    }

    #[test]
    fn range_lookup_is_empty() {
        let store = InMemoryBalStore::default();

        assert!(store.get_by_range(1, 10).unwrap().is_empty());
    }

    #[test]
    fn limited_lookup_returns_prefix() {
        let store = InMemoryBalStore::default();
        let hash0 = B256::random();
        let hash1 = B256::random();
        let hash2 = B256::random();
        let bal0 = Bytes::from_static(&[0xc1, 0x01]);
        let bal1 = Bytes::from_static(&[0xc1, 0x02]);
        let bal2 = Bytes::from_static(&[0xc1, 0x03]);

        store.insert(hash0, 1, bal0.clone()).unwrap();
        store.insert(hash1, 2, bal1.clone()).unwrap();
        store.insert(hash2, 3, bal2).unwrap();

        let limited = store
            .get_by_hashes_with_limit(
                &[hash0, hash1, hash2],
                GetBlockAccessListLimit::ResponseSizeSoftLimit(2),
            )
            .unwrap();

        assert_eq!(limited, vec![bal0, bal1]);
    }

    #[test]
    fn default_retention_prunes_old_bals() {
        let store = InMemoryBalStore::default();
        let old_hash = B256::random();
        let retained_hash = B256::random();
        let tip_hash = B256::random();
        let old_bal = Bytes::from_static(b"old");
        let retained_bal = Bytes::from_static(b"retained");
        let tip_bal = Bytes::from_static(b"tip");

        store.insert(old_hash, 1, old_bal).unwrap();
        store.insert(retained_hash, BAL_RETENTION_PERIOD_SLOTS, retained_bal.clone()).unwrap();
        store.insert(tip_hash, BAL_RETENTION_PERIOD_SLOTS + 2, tip_bal.clone()).unwrap();

        assert_eq!(
            store.get_by_hashes(&[old_hash, retained_hash, tip_hash]).unwrap(),
            vec![None, Some(retained_bal), Some(tip_bal)]
        );
    }

    #[test]
    fn unbounded_retention_keeps_old_bals() {
        let store = InMemoryBalStore::new(BalConfig::unbounded());
        let old_hash = B256::random();
        let tip_hash = B256::random();
        let old_bal = Bytes::from_static(b"old");
        let tip_bal = Bytes::from_static(b"tip");

        store.insert(old_hash, 1, old_bal.clone()).unwrap();
        store.insert(tip_hash, BAL_RETENTION_PERIOD_SLOTS + 1, tip_bal.clone()).unwrap();

        assert_eq!(
            store.get_by_hashes(&[old_hash, tip_hash]).unwrap(),
            vec![Some(old_bal), Some(tip_bal)]
        );
    }

    #[test]
    fn reinserting_hash_updates_number_index() {
        let store =
            InMemoryBalStore::new(BalConfig::with_in_memory_retention(PruneMode::Before(2)));
        let hash = B256::random();
        let bal = Bytes::from_static(b"bal");

        store.insert(hash, 1, Bytes::from_static(b"old")).unwrap();
        store.insert(hash, 2, bal.clone()).unwrap();

        assert_eq!(store.get_by_hashes(&[hash]).unwrap(), vec![Some(bal)]);
    }
}
