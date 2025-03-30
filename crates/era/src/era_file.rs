//! Era file handling
//!
//! See also <https://github.com/status-im/nimbus-eth2/blob/stable/docs/e2store.md#era-files>

use crate::{
    beacon_types::{CompressedBeaconState, CompressedSignedBeaconBlock},
    e2s_types::Entry,
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
