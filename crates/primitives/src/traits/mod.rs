//! Abstractions of primitive data types

pub mod block;
pub mod transaction;

pub use transaction::signed::SignedTransaction;
pub use block::{body::BlockBody, Block};

pub use alloy_consensus::BlockHeader;