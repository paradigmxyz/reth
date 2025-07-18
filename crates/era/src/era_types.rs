//! Era types for `.era` files
//!
//! See also <https://github.com/eth-clients/e2store-format-specs/blob/main/formats/era.md>

use crate::{
    consensus_types::{CompressedBeaconState, CompressedSignedBeaconBlock},
    e2s_types::Entry,
};
use alloy_primitives::U64;

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
///
/// See also <https://github.com/status-im/nimbus-eth2/blob/stable/docs/e2store.md#slotindex>.
#[derive(Debug, Clone)]
pub struct SlotIndex {
    /// Starting slot number
    pub starting_slot: U64,

    /// Offsets to data at each slot (0 indicates no data for that slot)
    pub offsets: Vec<U64>,
}
