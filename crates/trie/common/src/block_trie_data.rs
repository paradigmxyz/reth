//! Per-block hashed state and trie updates.

use crate::{updates::TrieUpdatesSorted, LazyHashedPostStateSorted};
use alloc::sync::Arc;

/// Trie data for a block, with hashed-state sorting allowed to finish in the background.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct BlockTrieData {
    /// Sorted hashed state, which may still be pending.
    pub hashed_state: LazyHashedPostStateSorted,
    /// Sorted trie updates, available immediately.
    pub trie_updates: Arc<TrieUpdatesSorted>,
}

#[cfg(all(test, feature = "serde"))]
mod tests {
    use super::*;
    use alloy_primitives::B256;
    use serde_json::json;

    #[test]
    fn preserves_sorted_trie_data_json() {
        // The object shape previously serialized by SortedTrieData and LazyTrieData.
        let legacy = json!({
            "hashed_state": {
                "accounts": [["0x0000000000000000000000000000000000000000000000000000000000000001", null]],
                "storages": {}
            },
            "trie_updates": { "account_nodes": [], "storage_tries": {} }
        });
        let data: BlockTrieData = serde_json::from_value(legacy.clone()).unwrap();

        assert_eq!(data.hashed_state.get().accounts, vec![(B256::with_last_byte(1), None)]);
        assert!(data.trie_updates.is_empty());
        assert_eq!(serde_json::to_value(&data).unwrap(), legacy);
    }
}
