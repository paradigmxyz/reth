//! Era types for `.era` files
//!
//! See also <https://github.com/eth-clients/e2store-format-specs/blob/main/formats/era.md>

use crate::{
    consensus_types::{CompressedBeaconState, CompressedSignedBeaconBlock},
    e2s_types::{Entry, SLOT_INDEX},
    E2sError,
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

    /// Convert to an [`Entry`] for writing to `e2store` format
    /// Format: starting-slot | offset1 | offset2 | ... | count
    /// so we encore the starting slot, each offset, and the count of offsets.
    pub fn to_entry(&self) -> Entry {
        let mut data = Vec::new();

        // Encode starting slot - 8 bytes
        data.extend_from_slice(&self.starting_slot.to_le_bytes());

        // Encode each offset - 8 bytes each
        data.extend(self.offsets.iter().flat_map(|offset| offset.to_le_bytes()));

        // Encode count - 8 bytes again
        let count = self.offsets.len() as u64;
        data.extend_from_slice(&count.to_le_bytes());

        Entry::new(SLOT_INDEX, data)
    }

    /// Create from an [`Entry`]
    pub fn from_entry(entry: &Entry) -> Result<Self, E2sError> {
        if !entry.is_slot_index() {
            return Err(E2sError::Ssz(format!(
                "Invalid entry type for SlotIndex: expected {:02x}{:02x}, got {:02x}{:02x}",
                SLOT_INDEX[0], SLOT_INDEX[1], entry.entry_type[0], entry.entry_type[1]
            )));
        }

        if entry.data.len() < 16 {
            return Err(E2sError::Ssz(
                "SlotIndex entry too short: need at least 16 bytes for starting_slot and count"
                    .to_string(),
            ));
        }

        // Read count from the last 8 bytes
        let count_bytes = &entry.data[entry.data.len() - 8..];
        let count = u64::from_le_bytes(
            count_bytes
                .try_into()
                .map_err(|_| E2sError::Ssz("Failed to read count bytes".to_string()))?,
        );

        // Total length should be: starting_slot + offsets + count
        let expected_len = 8 + (count as usize * 8) + 8;
        if entry.data.len() != expected_len {
            return Err(E2sError::Ssz(format!(
                "SlotIndex entry has incorrect length: expected {expected_len}, got {}",
                entry.data.len()
            )));
        }

        // Read starting slot from first 8 bytes
        let starting_slot = u64::from_le_bytes(
            entry.data[0..8]
                .try_into()
                .map_err(|_| E2sError::Ssz("Failed to read starting_slot bytes".to_string()))?,
        );

        // Extract all offsets
        let mut offsets = Vec::with_capacity(count as usize);
        for i in 0..count as usize {
            let start = 8 + (i * 8);
            let end = start + 8;
            let offset_bytes = &entry.data[start..end];
            let offset = u64::from_le_bytes(
                offset_bytes
                    .try_into()
                    .map_err(|_| E2sError::Ssz(format!("Failed to read offset {i} bytes")))?,
            );
            offsets.push(offset);
        }

        Ok(Self { starting_slot, offsets })
    }
}
