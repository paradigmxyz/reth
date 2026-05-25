use alloy_primitives::{map::B256Map, B256};
use reth_trie_common::{updates::TrieUpdatesSorted, HashedPostStateSorted};
use std::sync::Arc;

/// Source of per-account storage overlays for [`StorageOverlayIndex`].
pub(crate) trait StorageOverlayIndexSource {
    /// Returns every hashed address touched by this overlay and whether that storage overlay wipes
    /// lower-priority database or overlay contents for the address.
    fn storage_overlay_index_entries(&self) -> impl Iterator<Item = (B256, bool)> + '_;
}

impl StorageOverlayIndexSource for TrieUpdatesSorted {
    fn storage_overlay_index_entries(&self) -> impl Iterator<Item = (B256, bool)> + '_ {
        self.storage_tries_ref()
            .iter()
            .map(|(hashed_address, storage)| (*hashed_address, storage.is_deleted()))
    }
}

impl StorageOverlayIndexSource for HashedPostStateSorted {
    fn storage_overlay_index_entries(&self) -> impl Iterator<Item = (B256, bool)> + '_ {
        self.storages.iter().map(|(hashed_address, storage)| (*hashed_address, storage.is_wiped()))
    }
}

/// Precomputed lookup from hashed address to the overlay layers that contain storage for it.
pub(crate) type StorageOverlayIndex = B256Map<StorageOverlayIndexEntry>;

/// Incremental updates for a [`StorageOverlayIndex`].
pub(crate) trait StorageOverlayIndexMut {
    /// Adds a lower-priority overlay to this storage overlay index.
    fn append<T: StorageOverlayIndexSource>(&mut self, overlay_index: usize, overlay: &T);

    /// Adds a highest-priority overlay to this storage overlay index.
    fn prepend<T: StorageOverlayIndexSource>(&mut self, overlay: &T);
}

/// Index entry for one hashed address in a [`StorageOverlayIndex`].
#[derive(Clone, Debug, Default)]
pub(crate) struct StorageOverlayIndexEntry {
    /// Overlay indices that should be searched for a hashed address, ordered by precedence.
    pub(crate) indices: Arc<Vec<usize>>,
    /// Whether an overlay at one of [`Self::indices`] wipes lower-priority database contents.
    pub(crate) db_wiped: bool,
}

impl StorageOverlayIndexEntry {
    /// Builds a storage overlay index for the full overlay stack.
    pub(crate) fn new<T: StorageOverlayIndexSource>(overlays: &[Arc<T>]) -> StorageOverlayIndex {
        let mut index = StorageOverlayIndex::default();

        for (idx, overlay) in overlays.iter().enumerate() {
            index.append(idx, overlay.as_ref());
        }

        index
    }
}

impl StorageOverlayIndexMut for StorageOverlayIndex {
    fn append<T: StorageOverlayIndexSource>(&mut self, overlay_index: usize, overlay: &T) {
        for (hashed_address, wipes_db) in overlay.storage_overlay_index_entries() {
            let entry = self.entry(hashed_address).or_default();
            if entry.db_wiped {
                continue;
            }

            Arc::make_mut(&mut entry.indices).push(overlay_index);
            if wipes_db {
                entry.db_wiped = true;
            }
        }
    }

    fn prepend<T: StorageOverlayIndexSource>(&mut self, overlay: &T) {
        for entry in self.values_mut() {
            for idx in Arc::make_mut(&mut entry.indices) {
                *idx += 1;
            }
        }

        for (hashed_address, wipes_db) in overlay.storage_overlay_index_entries() {
            let entry = self.entry(hashed_address).or_default();
            let indices = Arc::make_mut(&mut entry.indices);

            if wipes_db {
                indices.clear();
                indices.push(0);
                entry.db_wiped = true;
            } else {
                indices.insert(0, 0);
            }
        }
    }
}
