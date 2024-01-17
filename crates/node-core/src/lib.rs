//! The core of the Ethereum node. Collection of utilities and libraries that are used by the node.

#![allow(missing_docs)]
#![allow(missing_debug_implementations)]
#![allow(dead_code)]

pub mod args;
pub mod cli;
pub mod dirs;
pub mod init;
pub mod utils;
pub mod version;

/// Re-exported from `reth_primitives`.
pub mod primitives {
    pub use reth_primitives::*;
}
