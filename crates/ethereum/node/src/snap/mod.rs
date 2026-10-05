//! Snap sync for the Ethereum node: handing downloaded state over to the staged pipeline.

mod handoff;

pub use handoff::{HandoffOutcome, RebuildOutcome, SnapHandoff};
