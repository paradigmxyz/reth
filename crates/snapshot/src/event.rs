use crate::SnapshotTargets;
use std::time::Duration;

/// An event emitted by a [Snapshotter][crate::Snapshotter].
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum SnapshotterEvent {
    /// Emitted when snapshotter finished running.
    Finished {
        /// Targets that were snapshotted
        targets: SnapshotTargets,
        /// Time it took to run the snapshotter
        elapsed: Duration,
    },
}
