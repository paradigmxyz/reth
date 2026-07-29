use crate::{constants::TABLE_SIZES, table::IndexTable};
use std::collections::HashMap;

/// In-memory cache of index tables, keyed by (first_block, table_size).
///
/// Tables can be regenerated from stored receipts if not present in the cache,
/// so no persistent storage is required. Only the most recent ~319 blocks of
/// tables are needed for async merging of higher-level tables.
#[derive(Debug, Default)]
pub struct IndexTableStore {
    tables: HashMap<(u64, u64), IndexTable>,
    fork_activation_block: u64,
}

impl IndexTableStore {
    pub fn new(fork_activation_block: u64) -> Self {
        Self { tables: HashMap::default(), fork_activation_block }
    }

    pub fn store(&mut self, table: IndexTable) {
        self.tables.insert((table.first_block, table.table_size), table);
    }

    pub fn get(&self, first_block: u64, table_size: u64) -> Option<&IndexTable> {
        self.tables.get(&(first_block, table_size))
    }

    /// Determine which tables need to be generated at a given block number.
    ///
    /// Returns a list of (first_block, table_size) pairs for tables whose
    /// generation deadline falls at this block.
    pub fn tables_to_generate_at(&self, block_number: u64) -> Vec<(u64, u64)> {
        if block_number < self.fork_activation_block {
            return Vec::new();
        }

        let mut result = Vec::new();

        for &table_size in &TABLE_SIZES {
            let delay = table_size / 4;
            if block_number < delay {
                continue;
            }

            let end_block = block_number - delay;
            if end_block % table_size != table_size - 1 {
                continue;
            }

            let first_block = end_block + 1 - table_size;
            if first_block < self.fork_activation_block {
                continue;
            }

            if self.get(first_block, table_size).is_none() {
                result.push((first_block, table_size));
            }
        }

        result
    }

    /// Merge four adjacent tables of a given size into one larger table.
    pub fn merge_level(&self, first_block: u64, small_size: u64) -> Option<IndexTable> {
        let mut tables: Vec<&IndexTable> = Vec::with_capacity(4);

        for i in 0..4 {
            let block = first_block + i * small_size;
            if let Some(table) = self.get(block, small_size) {
                tables.push(table);
            }
        }

        if tables.len() != 4 {
            return None;
        }

        Some(IndexTable::merge(&tables))
    }

    /// Clean up old tables that are no longer needed for merging.
    pub fn prune(&mut self, current_block: u64) {
        let max_table_size = TABLE_SIZES[TABLE_SIZES.len() - 1];
        let keep_threshold = current_block.saturating_sub(max_table_size * 2);

        self.tables.retain(|&(first_block, table_size), _| {
            let end_block = first_block + table_size;
            end_block > keep_threshold
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::IndexEntry;

    #[test]
    fn test_tables_to_generate_empty_before_fork() {
        let store = IndexTableStore::new(100);
        let pending = store.tables_to_generate_at(99);
        assert!(pending.is_empty());
    }

    #[test]
    fn test_tables_to_generate_level0() {
        let store = IndexTableStore::new(0);
        let pending = store.tables_to_generate_at(0);
        assert!(pending.contains(&(0, 1)));
    }

    #[test]
    fn test_tables_to_generate_level1() {
        let store = IndexTableStore::new(0);
        let pending = store.tables_to_generate_at(4);
        assert!(pending.contains(&(0, 4)));
    }

    #[test]
    fn test_store_and_get() {
        let mut store = IndexTableStore::new(0);
        let entry = IndexEntry::block_entry(0, alloy_primitives::B256::ZERO);
        let table = IndexTable::new(0, 1, vec![entry]);
        store.store(table);
        assert!(store.get(0, 1).is_some());
        assert!(store.get(0, 4).is_none());
    }

    #[test]
    fn test_prune_removes_old_tables() {
        let mut store = IndexTableStore::new(0);
        let entry = IndexEntry::block_entry(0, alloy_primitives::B256::ZERO);
        store.store(IndexTable::new(0, 1, vec![entry.clone()]));
        store.store(IndexTable::new(500, 1, vec![entry]));
        store.prune(600);
        assert!(store.get(0, 1).is_none());
        assert!(store.get(500, 1).is_some());
    }
}
