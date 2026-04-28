use alloy_primitives::{BlockHash, BlockNumber, Bytes};
use parking_lot::RwLock;
use reth_storage_api::{BalStore, GetBlockAccessListLimit};
use reth_storage_errors::provider::ProviderResult;
use std::{collections::HashMap, sync::Arc};

/// Basic in-memory BAL store keyed by block hash.
#[derive(Debug, Clone, Default)]
pub struct InMemoryBalStore {
    entries: Arc<RwLock<HashMap<BlockHash, Bytes>>>,
}

impl BalStore for InMemoryBalStore {
    fn insert(
        &self,
        block_hash: BlockHash,
        _block_number: BlockNumber,
        bal: Bytes,
    ) -> ProviderResult<()> {
        self.entries.write().insert(block_hash, bal);
        Ok(())
    }

    fn get_by_hashes(&self, block_hashes: &[BlockHash]) -> ProviderResult<Vec<Option<Bytes>>> {
        let entries = self.entries.read();
        let mut result = Vec::with_capacity(block_hashes.len());

        for hash in block_hashes {
            result.push(entries.get(hash).cloned());
        }

        Ok(result)
    }

    fn append_by_hashes_with_limit(
        &self,
        block_hashes: &[BlockHash],
        limit: GetBlockAccessListLimit,
        out: &mut Vec<Bytes>,
    ) -> ProviderResult<()> {
        let entries = self.entries.read();
        let mut size = 0;

        for hash in block_hashes {
            let bal = entries.get(hash).cloned().unwrap_or_else(|| Bytes::from_static(&[0xc0]));
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
}
