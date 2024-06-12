use std::sync::Arc;

use parking_lot::RwLock;
use reth_provider::ProviderFactory;
use crate::tree::TreeState;

/// Writes parts of the tree to the database
pub struct BackgroundWriter<DB> {
    /// The db / static file provider to use
    provider: ProviderFactory<DB>,
    /// The locked tree state
    tree_state: Arc<RwLock<TreeState>>,
}

impl<DB> BackgroundWriter<DB> {
    /// Produces a clone of the tree state used for writing
    pub fn tree_state(&self) -> TreeState {
        let reader = self.tree_state.read();
        reader.clone()
    }

    /// Writes the cloned tree state to the database
    pub fn write(&self) {
        todo!("need tree state to write")
    }
}
