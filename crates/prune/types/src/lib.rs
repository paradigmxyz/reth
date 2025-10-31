//! Commonly used types for prune usage.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

mod checkpoint;
mod event;
mod mode;
mod pruner;
mod segment;
mod target;

pub use checkpoint::PruneCheckpoint;
pub use event::PrunerEvent;
pub use mode::PruneMode;
pub use pruner::{
    PruneInterruptReason, PruneProgress, PrunedSegmentInfo, PrunerOutput, SegmentOutput,
    SegmentOutputCheckpoint,
};
pub use segment::{PrunePurpose, PruneSegment, PruneSegmentError, PRUNE_SEGMENTS};
pub use target::{PruneModes, UnwindTargetPrunedError, MINIMUM_PRUNING_DISTANCE};
