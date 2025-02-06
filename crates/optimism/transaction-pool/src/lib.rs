//! Optimism transaction pool implementation.
//!
//! This crate provides the transaction pool implementation for Optimism nodes.
//! It extends the base Ethereum transaction pool to handle Optimism-specific transaction types
//! and validation rules.

pub mod txpool;
pub use txpool::*;
