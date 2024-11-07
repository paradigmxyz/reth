//! Abstractions of primitive data types

pub mod block;
pub mod transaction;

pub use block::{body::BlockBody, Block};
pub use transaction::signed::SignedTransaction;

pub use alloy_consensus::BlockHeader;
