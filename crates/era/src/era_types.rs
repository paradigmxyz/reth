//! Era file handling
//!
//! See also <https://github.com/status-im/nimbus-eth2/blob/stable/docs/e2store.md#era-files>

use crate::{
    beacon_types::{CompressedBeaconState, CompressedSignedBeaconBlock},
    e2s_types::{E2sError, Entry, SLOT_INDEX},
};
use alloy_primitives::hex;

/// [`SlotIndex`] records store offsets to data at specific slots
/// from the beginning of the index record
/// to the beginning of the corresponding data at that slot.
///
/// Format:
/// `starting-slot | index | index | index ... | count`
///
/// See also <https://github.com/status-im/nimbus-eth2/blob/stable/docs/e2store.md#slotindex>
#[derive(Debug, Clone)]
pub struct SlotIndex {
    /// Starting slot number
    pub starting_slot: i64,

    /// Offsets to data at each slot
    pub offsets: Vec<i64>,
}

impl SlotIndex {
    /// Create a new [`SlotIndex`]
    pub fn new(starting_slot: i64, offsets: Vec<i64>) -> Self {
        Self { starting_slot, offsets }
    }

    /// Get the offset for a specific slot
    pub fn offset_for_slot(&self, slot: i64) -> Option<i64> {
        let index = (slot - self.starting_slot) as usize;
        self.offsets.get(index).copied()
    }

    /// Convert to an [`Entry`] for storage in an e2store file
    pub fn to_entry(&self) -> Entry {
        // Format: starting-slot | index | index | index ... | count
        let mut data = Vec::with_capacity(8 + self.offsets.len() * 8 + 8);

        // Add starting slot
        data.extend_from_slice(&self.starting_slot.to_le_bytes());

        // Add all offsets
        for offset in &self.offsets {
            data.extend_from_slice(&offset.to_le_bytes());
        }

        // Add count
        data.extend_from_slice(&(self.offsets.len() as i64).to_le_bytes());

        Entry::new(SLOT_INDEX, data)
    }

    /// Create from an [`Entry`]
    pub fn from_entry(entry: &Entry) -> Result<Self, E2sError> {
        if entry.entry_type != SLOT_INDEX {
            return Err(E2sError::Ssz(format!(
                "Invalid entry type for SlotIndex: expected {:02x}{:02x}, got {:02x}{:02x}",
                SLOT_INDEX[0], SLOT_INDEX[1], entry.entry_type[0], entry.entry_type[1]
            )));
        }

        if entry.data.len() < 16 {
            return Err(E2sError::Ssz(String::from(
                "SlotIndex entry too short to contain starting slot and count",
            )));
        }

        // Extract starting slot = first 8 bytes
        let mut starting_slot_bytes = [0u8; 8];
        starting_slot_bytes.copy_from_slice(&entry.data[0..8]);
        let starting_slot = i64::from_le_bytes(starting_slot_bytes);

        // Extract count = last 8 bytes
        let mut count_bytes = [0u8; 8];
        count_bytes.copy_from_slice(&entry.data[entry.data.len() - 8..]);
        let count = i64::from_le_bytes(count_bytes) as usize;

        // Verify that the entry has the correct size
        let expected_size = 8 + count * 8 + 8;
        if entry.data.len() != expected_size {
            return Err(E2sError::Ssz(format!(
                "SlotIndex entry has incorrect size: expected {}, got {}",
                expected_size,
                entry.data.len()
            )));
        }

        // Extract all offsets
        let mut offsets = Vec::with_capacity(count);
        for i in 0..count {
            let start = 8 + i * 8;
            let end = start + 8;
            let mut offset_bytes = [0u8; 8];
            offset_bytes.copy_from_slice(&entry.data[start..end]);
            offsets.push(i64::from_le_bytes(offset_bytes));
        }

        Ok(Self { starting_slot, offsets })
    }
}

/// Era file identifier
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EraId {
    /// Network configuration name
    pub config_name: String,

    /// First era number in file
    pub era_number: u32,

    /// Number of eras in the file
    pub era_count: u32,

    /// Short historical root
    pub short_root: [u8; 4],
}

impl EraId {
    /// Create a new [`EraId`]
    pub fn new(
        config_name: impl Into<String>,
        era_number: u32,
        era_count: u32,
        short_root: [u8; 4],
    ) -> Self {
        Self { config_name: config_name.into(), era_number, era_count, short_root }
    }

    /// Convert to file name following the era file naming convention :
    /// `config-name>-<era-number>-<era-count>-<short-historical-root>.era`
    /// See also <https://github.com/status-im/nimbus-eth2/blob/stable/docs/e2store.md#file-name>
    pub fn to_file_name(&self) -> String {
        format!(
            "{}-{:05}-{:02}-{}.era",
            self.config_name,
            self.era_number,
            self.era_count,
            hex::encode(self.short_root)
        )
    }
}

/// Group in an Era file
///
/// Format: `Version | block* | era-state | other-entries* | slot-index(block)? | slot-index(state)`
#[derive(Debug)]
pub struct EraGroup {
    /// Blocks in this era group
    pub blocks: Vec<CompressedSignedBeaconBlock>,

    /// State at the end of this era
    pub state: CompressedBeaconState,

    /// Other entries that don't fit into the standard categories
    pub other_entries: Vec<Entry>,

    /// Block index, optional, omitted for genesis era
    pub block_index: Option<SlotIndex>,

    /// State index
    pub state_index: SlotIndex,
}

impl EraGroup {
    /// Create a new [`EraGroup`]
    pub fn new(
        state: CompressedBeaconState,
        state_index: SlotIndex,
        blocks: Vec<CompressedSignedBeaconBlock>,
        block_index: Option<SlotIndex>,
    ) -> Self {
        Self { blocks, state, other_entries: Vec::new(), block_index, state_index }
    }

    /// Add another entry to this group
    pub fn add_entry(&mut self, entry: Entry) {
        self.other_entries.push(entry);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::e2s_types::SLOT_INDEX;

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
}
