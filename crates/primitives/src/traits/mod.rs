//! Abstractions of primitive data types

pub mod block;
pub mod signed;

pub use signed::SignedTransaction;
pub use block::{body::BlockBody, Block};

pub use alloy_consensus::BlockHeader;