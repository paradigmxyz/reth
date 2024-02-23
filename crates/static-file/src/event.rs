use crate::StaticFileTargets;
use std::time::Duration;

/// An event emitted by a [Snapshotter][crate::Snapshotter].
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum StaticFileProducerEvent {
    /// Emitted when snapshotter finished running.
    Finished {
        /// Targets that were snapshotted
        targets: StaticFileTargets,
        /// Time it took to run the snapshotter
        elapsed: Duration,
    },
}
