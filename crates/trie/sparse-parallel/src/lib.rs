//! The implementation of parallel sparse MPT.

#![cfg_attr(not(test), warn(unused_crate_dependencies))]

mod trie;
pub use trie::*;

mod lower;
use lower::*;
