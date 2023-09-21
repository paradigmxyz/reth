mod error;
mod snapshotter;

pub use error::SnapshotterError;
pub use snapshotter::{
    HighestSnapshots, HighestSnapshotsTracker, SnapshotTargets, Snapshotter, SnapshotterResult,
    SnapshotterWithResult,
};
