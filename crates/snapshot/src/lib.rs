mod error;
mod snapshotter;

pub use error::SnapshotterError;
pub use snapshotter::{SnapshotTargets, Snapshotter, SnapshotterResult, SnapshotterWithResult};
