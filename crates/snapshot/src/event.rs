use crate::SnapshotTargets;
use std::time::Duration;

/// An event emitted by a [Pruner][crate::Pruner].
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum SnapshotterEvent {
    /// Emitted when snapshotter finished running.
    Finished { targets: SnapshotTargets, elapsed: Duration },
}
