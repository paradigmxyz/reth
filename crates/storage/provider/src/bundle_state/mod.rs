//! Bundle state module.
//! This module contains all the logic related to bundle state.

mod state_changes;
mod state_reverts;

pub use state_changes::StateChanges;
pub use state_reverts::{StateReverts, StorageRevertsIter};
