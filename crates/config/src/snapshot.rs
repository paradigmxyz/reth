use reth_primitives::SnapshotSegment;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Snapshots configuration.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Serialize)]
pub struct SnapshotConfig {
    /// Snapshot configuration for every segment of the data that can be snapshotted.
    segments: HashMap<SnapshotSegment, bool>,
}
