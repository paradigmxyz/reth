//! Abstractions of primitive data types

pub mod block;

pub use block::{body::BlockBody, Block};

pub use alloy_consensus::BlockHeader;
