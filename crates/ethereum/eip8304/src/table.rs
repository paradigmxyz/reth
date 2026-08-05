use alloy_primitives::B256;
use sha2::{Digest, Sha256};

use crate::entry::IndexEntry;

/// An index table containing sorted entries for a block range.
#[derive(Debug, Clone)]
pub struct IndexTable {
    pub entries: Vec<IndexEntry>,
    pub first_block: u64,
    pub table_size: u64,
}

impl IndexTable {
    pub fn new(first_block: u64, table_size: u64, entries: Vec<IndexEntry>) -> Self {
        Self { entries, first_block, table_size }
    }

    pub fn entry_count(&self) -> u64 {
        self.entries.len() as u64
    }

    /// Compute the SSZ `hash_tree_root(List[Hash32, entry_count])` for this table.
    ///
    /// Algorithm:
    /// 1. SHA2-256 hash each encoded entry -> leaves
    /// 2. Pad to next power of two with zero hashes
    /// 3. Build binary Merkle tree bottom-up (SHA256(left ++ right))
    /// 4. mix_in_length: SHA256(merkle_root ++ uint256_le(entry_count))
    ///
    /// ## Depth
    ///
    /// The EIP states `List[Hash32, entry_count]` at line 170, i.e. the SSZ *limit*
    /// equals the actual entry count. Under that reading padding to
    /// `next_power_of_two(entry_count)` is the correct chunk count.
    ///
    /// The EIP abstract (line 16) and `Index tables` section (line 51) describe
    /// a **fixed-depth** binary tree, which in canonical SSZ would require a
    /// compile-time constant capacity N in `List[Hash32, N]`. No such constant is
    /// defined anywhere in the specification (the parameter table lists only
    /// `TABLE_SIZES`, `TABLES_PER_LEVEL`, and addresses).
    ///
    /// We therefore implement the literal reading (limit == actual count), which is
    /// the only implementable interpretation today. If the EIP later adopts a fixed
    /// capacity the pad target (`next_power_of_two`) is the single point of change.
    ///
    /// ## Empty tables
    ///
    /// Canonical SSZ `hash_tree_root(List[Hash32, 0])` =
    /// `mix_in_length(zero_hash(0), 0)` = `SHA256(0x00×64)`. The bytecode
    /// reserves `B256::ZERO` as the "slot unset" sentinel (`get` reverts on a
    /// zero slot), so an empty table must not hash to zero.
    ///
    /// In practice a level-0 table is never empty (the delayed parent block entry
    /// is always present when `block_number > 0`), so only genesis block 0 with
    /// zero transactions hits this path.
    pub fn compute_root(&self) -> B256 {
        let count = self.entries.len();
        if count == 0 {
            return B256::from(sha256_hash(&[0u8; 64]));
        }

        let mut leaves: Vec<[u8; 32]> =
            self.entries.iter().map(|e| sha256_hash(&e.encode())).collect();

        let padded_len = count.next_power_of_two();
        leaves.resize(padded_len, [0u8; 32]);

        while leaves.len() > 1 {
            let mut next_level = Vec::with_capacity(leaves.len() / 2);
            for pair in leaves.chunks_exact(2) {
                let mut data = [0u8; 64];
                data[..32].copy_from_slice(&pair[0]);
                data[32..].copy_from_slice(&pair[1]);
                next_level.push(sha256_hash(&data));
            }
            leaves = next_level;
        }

        let merkle_root = leaves[0];

        let mut mix = [0u8; 64];
        mix[..32].copy_from_slice(&merkle_root);
        let length_bytes = (count as u64).to_le_bytes();
        mix[32..40].copy_from_slice(&length_bytes);

        B256::from(sha256_hash(&mix))
    }

    /// Merge multiple sorted index tables into one.
    /// All tables must cover adjacent block ranges.
    pub fn merge(tables: &[&Self]) -> Self {
        let mut first_block = u64::MAX;
        let mut total_blocks = 0u64;
        let mut total_entries = 0usize;
        for t in tables {
            first_block = first_block.min(t.first_block);
            total_blocks += t.table_size;
            total_entries += t.entries.len();
        }

        let mut merged = Vec::with_capacity(total_entries);

        let mut indices: Vec<usize> = vec![0; tables.len()];

        loop {
            let mut smallest_idx: Option<usize> = None;
            for (i, table) in tables.iter().enumerate() {
                if indices[i] < table.entries.len() {
                    match smallest_idx {
                        None => smallest_idx = Some(i),
                        Some(current) => {
                            if table.entries[indices[i]] < tables[current].entries[indices[current]]
                            {
                                smallest_idx = Some(i);
                            }
                        }
                    }
                }
            }
            match smallest_idx {
                None => break,
                Some(idx) => {
                    merged.push(tables[idx].entries[indices[idx]].clone());
                    indices[idx] += 1;
                }
            }
        }

        Self { entries: merged, first_block, table_size: total_blocks }
    }
}

fn sha256_hash(data: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::IndexEntry;
    use alloy_primitives::b256;

    fn example_table() -> IndexTable {
        let hash1 = b256!("42f66a2e9f9c68e223e8d826145d7cfacb00520dba6a9555803121de29790b65");
        let hash2 = b256!("66f42ef12b140e8004ad39a760191457f485204ee6c9f990c33f14014e521f20");
        let entries = vec![IndexEntry::block_entry(40, hash1), IndexEntry::block_entry(42, hash2)];
        IndexTable::new(40, 4, entries)
    }

    #[test]
    fn test_empty_table_root() {
        let table = IndexTable::new(0, 1, vec![]);
        // canonical SSZ hash_tree_root(List[Hash32, 0]) =
        // mix_in_length(zero_hash(0), 0) = SHA256(0x00×64)
        assert_eq!(table.compute_root(), B256::from(sha256_hash(&[0u8; 64])));
    }

    #[test]
    fn test_empty_table_root_is_not_zero_sentinel() {
        // The index contract treats B256::ZERO as an uninitialized ring-buffer
        // slot; an empty table root must not collide with that sentinel.
        let table = IndexTable::new(0, 1, vec![]);
        assert_ne!(table.compute_root(), B256::ZERO);
    }

    #[test]
    fn test_single_entry_table_root() {
        let hash = b256!("42f66a2e9f9c68e223e8d826145d7cfacb00520dba6a9555803121de29790b65");
        let entry = IndexEntry::block_entry(40, hash);
        let table = IndexTable::new(40, 1, vec![entry]);
        let root = table.compute_root();
        assert_ne!(root, B256::ZERO);
    }

    #[test]
    fn test_two_entry_table_root_is_deterministic() {
        let table = example_table();
        let root1 = table.compute_root();
        let root2 = table.compute_root();
        assert_eq!(root1, root2);
    }

    #[test]
    fn test_different_entries_different_roots() {
        let table1 = example_table();
        let mut entries = table1.entries.clone();
        entries[0] = IndexEntry::block_entry(41, B256::repeat_byte(0xff));
        let table2 = IndexTable::new(40, 4, entries);
        assert_ne!(table1.compute_root(), table2.compute_root());
    }

    #[test]
    fn test_entry_count_affects_root() {
        let mut entries = vec![];
        for i in 0..4 {
            entries.push(IndexEntry::block_entry(40 + i, B256::repeat_byte(i as u8)));
        }
        let mut entries_3 = entries[..3].to_vec();
        let table_3 = IndexTable::new(40, 3, entries_3);
        let table_4 = IndexTable::new(40, 4, entries);

        let root_3 = table_3.compute_root();
        let root_4 = table_4.compute_root();
        assert_ne!(root_3, root_4);
    }

    #[test]
    fn test_merge_tables() {
        let e1 = IndexEntry::block_entry(40, B256::repeat_byte(0x01));
        let e2 = IndexEntry::block_entry(41, B256::repeat_byte(0x02));
        let e3 = IndexEntry::block_entry(42, B256::repeat_byte(0x03));
        let e4 = IndexEntry::block_entry(43, B256::repeat_byte(0x04));

        let t1 = IndexTable::new(40, 2, vec![e1.clone(), e2.clone()]);
        let t2 = IndexTable::new(42, 2, vec![e3.clone(), e4.clone()]);

        let merged = IndexTable::merge(&[&t1, &t2]);
        assert_eq!(merged.first_block, 40);
        assert_eq!(merged.table_size, 4);
        assert_eq!(merged.entries.len(), 4);
    }

    #[test]
    fn test_merge_preserves_sorting() {
        let e1 = IndexEntry::block_entry(41, B256::repeat_byte(0x02));
        let e2 = IndexEntry::block_entry(40, B256::repeat_byte(0x01));
        let t1 = IndexTable::new(40, 1, vec![e1.clone()]);
        let t2 = IndexTable::new(41, 1, vec![e2.clone()]);

        let merged = IndexTable::merge(&[&t1, &t2]);

        for i in 1..merged.entries.len() {
            assert!(merged.entries[i - 1] <= merged.entries[i]);
        }
    }
}
