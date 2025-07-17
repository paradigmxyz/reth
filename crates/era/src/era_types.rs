//! Era types for `.era` files
//!
//! See also <https://github.com/eth-clients/e2store-format-specs/blob/main/formats/era.md>

use crate::e2s_types::Entry;
use crate::consensus_types::{CompressedSignedBeaconBlock, CompressedBeaconState};

/// `CompressedSignedBeaconBlock` record type: [0x01, 0x00]
pub const COMPRESSED_SIGNED_BEACON_BLOCK: [u8; 2] = [0x01, 0x00];

/// `CompressedBeaconState` record type: [0x02, 0x00]
pub const COMPRESSED_BEACON_STATE: [u8; 2] = [0x02, 0x00];

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
    pub block_slot_index: Option<SlotIndex>,

    /// State slot index
    pub state_slot_index: SlotIndex,
}

/// [`SlotIndex`] records store offsets to data at specific slots
/// from the beginning of the index record to the beginning of the corresponding data.
///
/// Format: `starting-slot | index | index | index ... | count`
#[derive(Debug, Clone)]
pub struct SlotIndex {
    /// Starting slot number
    pub starting_slot: u64,

    /// Offsets to data at each slot (0 indicates no data for that slot)
    pub offsets: Vec<i64>,
}
