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
    pub fn is_genesis(&self) -> bool {
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
#[derive(Debug, Clone)]
pub struct SlotIndex {
    /// Starting slot number
    pub starting_slot: u64,

    /// Offsets to data at each slot
    /// 0 indicates no data for that slot
    pub offsets: Vec<u64>,
}

impl SlotIndex {
    /// Create a new slot index
    pub const fn new(starting_slot: u64, offsets: Vec<u64>) -> Self {
        Self { starting_slot, offsets }
    }

    /// Get the number of slots covered by this index
    pub fn slot_count(&self) -> usize {
        self.offsets.len()
    }

    /// Get the offset for a specific slot
    pub fn get_offset(&self, slot_index: usize) -> Option<u64> {
        self.offsets.get(slot_index).copied()
    }

    /// Check if a slot has data - non-zero offset
    pub fn has_data_at_slot(&self, slot_index: usize) -> bool {
        self.get_offset(slot_index).is_some_and(|offset| offset != 0)
    }
}

impl IndexEntry for SlotIndex {
    fn new(starting_number: u64, offsets: Vec<u64>) -> Self {
        Self::new(starting_number, offsets)
    }

    fn entry_type() -> [u8; 2] {
        SLOT_INDEX
    }

    fn starting_number(&self) -> u64 {
        self.starting_slot
    }

    fn offsets(&self) -> &[u64] {
        &self.offsets
    }
}
