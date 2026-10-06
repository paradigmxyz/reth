//! Carries the downloaded state forward through verified block access lists.
//!
//! The chain moves on while ranges download, so state anchored at the pivot is already behind by
//! the time it is complete. [EIP-8189](https://eips.ethereum.org/EIPS/eip-8189#application-order)
//! closes that distance by applying the lists of the blocks after the pivot in strict block order,
//! each recording the final value of every field its block changes.
//!
//! Progress is the last block applied, kept as a number and hash so the next list is only taken
//! from its child, and committed with the state that list changes. A block whose list no peer
//! serves holds back every block after it rather than leaving a hole in the applied sequence.

mod apply;
mod download;
mod store;

pub use apply::{BalStateUpdate, DownloadedAccount};
pub use download::{
    BlockAccessListCatchUp, CatchUpStep, DEFAULT_BAL_RESPONSE_BYTES, DEFAULT_CATCH_UP_BLOCKS,
};
pub use store::{CatchUpProgress, SnapCatchUpStore};
