//! Local node implementation

use super::types::*;
use std::sync::RwLock;

/// P2P node
#[derive(Debug)]
pub struct Node {
    /// The current node status
    pub status: RwLock<Status>,
}

impl Node {
    /// Stream peer messages
    pub async fn stream_messages<T>(&self, _request: T) -> MessageStream {
        todo!()
    }

    /// Update current node status
    pub async fn update_status(&self, _: Option<Status>) {
        todo!()
    }
}
