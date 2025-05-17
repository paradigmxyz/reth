//! This example shows how implement a custom node.
//!
//! A node consists of:
//! - primitives: block,header,transactions
//! - components: network,pool,evm
//! - engine: advances the node

#![cfg_attr(not(test), warn(unused_crate_dependencies))]

pub use node::*;

pub mod chainspec;
pub mod consensus;
pub mod engine;
pub mod engine_api;
pub mod evm;
pub mod executor;
pub mod network;
pub mod payload;
pub mod pool;
pub mod primitives;

mod node;
