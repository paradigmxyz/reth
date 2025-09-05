//! Era types for `.era` files
//!
//! See also <https://github.com/eth-clients/e2store-format-specs/blob/main/formats/era.md>

use crate::{
    consensus_types::{CompressedBeaconState, CompressedSignedBeaconBlock},
    e2s_types::{Entry, IndexEntry, SLOT_INDEX},
};

/// Era file content group
///
/// Format: `Version | block* | era-state | other-entries* | slot-index(block)? | slot-index(state)`
/// See also <https://github.com/eth-clients/e2store-format-specs/blob/main/formats/era.md#structure>
#[derive(Debug)]
pub struct EraGroup {
    /// Group including all blocks leading up to the era transition in slot order
    pub blocks: Vec<CompressedSignedBeaconBlock>,

    /// State in the era transition slot
    pub era_state: CompressedBeaconState,

    /// Other entries that don't fit into standard categories
    pub other_entries: Vec<Entry>,

    /// Block slot index, omitted for genesis era
    pub slot_index: Option<SlotIndex>,

    /// State slot index
    pub state_slot_index: SlotIndex,
}

impl EraGroup {
    /// Create a new era group
    pub const fn new(
        blocks: Vec<CompressedSignedBeaconBlock>,
        era_state: CompressedBeaconState,
        state_slot_index: SlotIndex,
    ) -> Self {
        Self { blocks, era_state, other_entries: Vec::new(), slot_index: None, state_slot_index }
    }

    /// Create a new era group with block slot index
    pub const fn with_block_index(
        blocks: Vec<CompressedSignedBeaconBlock>,
        era_state: CompressedBeaconState,
        slot_index: SlotIndex,
        state_slot_index: SlotIndex,
    ) -> Self {
        Self {
            blocks,
            era_state,
            other_entries: Vec::new(),
            slot_index: Some(slot_index),
            state_slot_index,
        }
    }

    /// Check if this is a genesis era - no blocks yet
    pub const fn is_genesis(&self) -> bool {
        self.blocks.is_empty() && self.slot_index.is_none()
    }

    /// Add another entry to this group
    pub fn add_entry(&mut self, entry: Entry) {
        self.other_entries.push(entry);
    }
}

/// [`SlotIndex`] records store offsets to data at specific slots
/// from the beginning of the index record to the beginning of the corresponding data.
///
/// Format: `starting-slot | index | index | index ... | count`
///
/// See also <https://github.com/status-im/nimbus-eth2/blob/stable/docs/e2store.md#slotindex>.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotIndex {
    /// Starting slot number
    pub starting_slot: u64,

    /// Offsets to data at each slot
    /// 0 indicates no data for that slot
    pub offsets: Vec<i64>,
}

impl SlotIndex {
    /// Create a new slot index
    pub const fn new(starting_slot: u64, offsets: Vec<i64>) -> Self {
        Self { starting_slot, offsets }
    }

    /// Get the number of slots covered by this index
    pub const fn slot_count(&self) -> usize {
        self.offsets.len()
    }

    /// Get the offset for a specific slot
    pub fn get_offset(&self, slot_index: usize) -> Option<i64> {
        self.offsets.get(slot_index).copied()
    }

    /// Check if a slot has data - non-zero offset
    pub fn has_data_at_slot(&self, slot_index: usize) -> bool {
        self.get_offset(slot_index).is_some_and(|offset| offset != 0)
    }
}

impl IndexEntry for SlotIndex {
    fn new(starting_number: u64, offsets: Vec<i64>) -> Self {
        Self { starting_slot: starting_number, offsets }
    }

    fn entry_type() -> [u8; 2] {
        SLOT_INDEX
    }

    fn starting_number(&self) -> u64 {
        self.starting_slot
    }

    fn offsets(&self) -> &[i64] {
        &self.offsets
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        e2s_types::{Entry, IndexEntry},
        test_utils::{create_beacon_block, create_beacon_state},
    };

    #[test]
    fn test_slot_index_roundtrip() {
        let starting_slot = 1000;
        let offsets = vec![100, 200, 300, 400, 500];

        let slot_index = SlotIndex::new(starting_slot, offsets.clone());

        let entry = slot_index.to_entry();

        // Validate entry type
        assert_eq!(entry.entry_type, SLOT_INDEX);

        // Convert back to slot index
        let recovered = SlotIndex::from_entry(&entry).unwrap();

        // Verify fields match
        assert_eq!(recovered.starting_slot, starting_slot);
        assert_eq!(recovered.offsets, offsets);
    }
    #[test]
    fn test_slot_index_basic_operations() {
        let starting_slot = 2000;
        let offsets = vec![100, 200, 300];

        let slot_index = SlotIndex::new(starting_slot, offsets);

        assert_eq!(slot_index.slot_count(), 3);
        assert_eq!(slot_index.starting_slot, 2000);
    }

    #[test]
    fn test_slot_index_empty_slots() {
        let starting_slot = 1000;
        let offsets = vec![100, 0, 300, 0, 500];

        let slot_index = SlotIndex::new(starting_slot, offsets);

        // Test that empty slots return false for has_data_at_slot
        // slot 1000: offset 100
        assert!(slot_index.has_data_at_slot(0));
        // slot 1001: offset 0 - empty
        assert!(!slot_index.has_data_at_slot(1));
        // slot 1002: offset 300
        assert!(slot_index.has_data_at_slot(2));
        // slot 1003: offset 0 - empty
        assert!(!slot_index.has_data_at_slot(3));
        // slot 1004: offset 500
        assert!(slot_index.has_data_at_slot(4));
    }

    #[test]
    fn test_era_group_basic_construction() {
        let blocks =
            vec![create_beacon_block(10), create_beacon_block(15), create_beacon_block(20)];
        let era_state = create_beacon_state(50);
        let state_slot_index = SlotIndex::new(1000, vec![100, 200, 300]);

        let era_group = EraGroup::new(blocks, era_state, state_slot_index);

        // Verify initial state
        assert_eq!(era_group.blocks.len(), 3);
        assert_eq!(era_group.other_entries.len(), 0);
        assert_eq!(era_group.slot_index, None);
        assert_eq!(era_group.state_slot_index.starting_slot, 1000);
        assert_eq!(era_group.state_slot_index.offsets, vec![100, 200, 300]);
    }

    #[test]
    fn test_era_group_with_block_index() {
        let blocks = vec![create_beacon_block(10), create_beacon_block(15)];
        let era_state = create_beacon_state(50);
        let block_slot_index = SlotIndex::new(500, vec![50, 100]);
        let state_slot_index = SlotIndex::new(1000, vec![200, 300]);

        let era_group =
            EraGroup::with_block_index(blocks, era_state, block_slot_index, state_slot_index);

        // Verify state with block index
        assert_eq!(era_group.blocks.len(), 2);
        assert_eq!(era_group.other_entries.len(), 0);
        assert!(era_group.slot_index.is_some());

        let block_index = era_group.slot_index.as_ref().unwrap();
        assert_eq!(block_index.starting_slot, 500);
        assert_eq!(block_index.offsets, vec![50, 100]);

        assert_eq!(era_group.state_slot_index.starting_slot, 1000);
        assert_eq!(era_group.state_slot_index.offsets, vec![200, 300]);
    }

    #[test]
    fn test_era_group_genesis_check() {
        // Genesis era - no blocks, no block slot index
        let era_state = create_beacon_state(50);
        let state_slot_index = SlotIndex::new(0, vec![100]);

        let genesis_era = EraGroup::new(vec![], era_state, state_slot_index);
        assert!(genesis_era.is_genesis());

        // Non-genesis era - has blocks
        let blocks = vec![create_beacon_block(10)];
        let era_state = create_beacon_state(50);
        let state_slot_index = SlotIndex::new(1000, vec![100]);

        let normal_era = EraGroup::new(blocks, era_state, state_slot_index);
        assert!(!normal_era.is_genesis());

        // Non-genesis era - has block slot index
        let era_state = create_beacon_state(50);
        let block_slot_index = SlotIndex::new(500, vec![50]);
        let state_slot_index = SlotIndex::new(1000, vec![100]);

        let era_with_index =
            EraGroup::with_block_index(vec![], era_state, block_slot_index, state_slot_index);
        assert!(!era_with_index.is_genesis());
    }

    #[test]
    fn test_era_group_add_entries() {
        let blocks = vec![create_beacon_block(10)];
        let era_state = create_beacon_state(50);
        let state_slot_index = SlotIndex::new(1000, vec![100]);

        // Create and verify group
        let mut era_group = EraGroup::new(blocks, era_state, state_slot_index);
        assert_eq!(era_group.other_entries.len(), 0);

        // Create custom entries with different types
        let entry1 = Entry::new([0x01, 0x01], vec![1, 2, 3, 4]);
        let entry2 = Entry::new([0x02, 0x02], vec![5, 6, 7, 8]);

        // Add those entries
        era_group.add_entry(entry1);
        era_group.add_entry(entry2);

        // Verify entries were added correctly
        assert_eq!(era_group.other_entries.len(), 2);
        assert_eq!(era_group.other_entries[0].entry_type, [0x01, 0x01]);
        assert_eq!(era_group.other_entries[0].data, vec![1, 2, 3, 4]);
        assert_eq!(era_group.other_entries[1].entry_type, [0x02, 0x02]);
        assert_eq!(era_group.other_entries[1].data, vec![5, 6, 7, 8]);
    }

    #[test]
    fn test_index_with_negative_offset() {
        let mut data = Vec::new();
        data.extend_from_slice(&0u64.to_le_bytes());
        data.extend_from_slice(&(-1024i64).to_le_bytes());
        data.extend_from_slice(&0i64.to_le_bytes());
        data.extend_from_slice(&2i64.to_le_bytes());

        let entry = Entry::new(SLOT_INDEX, data);
        let index = SlotIndex::from_entry(&entry).unwrap();
        let parsed_offset = index.offsets[0];
        assert_eq!(parsed_offset, -1024);
    }
}
