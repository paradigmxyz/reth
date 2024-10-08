//! Abstractions of primitive data types

pub mod block;

pub use block::{body::BlockBody, Block, Header};

pub use alloy_consensus::BlockHeader;
