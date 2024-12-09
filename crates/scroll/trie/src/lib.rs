//! Fast binary Merkle-Patricia Trie (zktrie) state root calculator and proof generator for
//! prefix-sorted bits.

#![cfg_attr(not(doctest), doc = include_str!("../assets/zktrie.md"))]

#[macro_use]
#[allow(unused_imports)]
extern crate alloc;

mod branch;
mod hash_builder;
mod leaf;
mod sub_tree;

pub use hash_builder::HashBuilder;

use reth_scroll_primitives::poseidon::Fr;

/// The hashing domain for leaf nodes.
pub const LEAF_NODE_DOMAIN: Fr = Fr::from_raw([4, 0, 0, 0]);

/// The hashing domain for a branch node with two terminal children.
pub const BRANCH_NODE_LTRT_DOMAIN: Fr = Fr::from_raw([6, 0, 0, 0]);

/// The hashing domain for a branch node with a left terminal child and a right branch child.
pub const BRANCH_NODE_LTRB_DOMAIN: Fr = Fr::from_raw([7, 0, 0, 0]);

/// The hashing domain for a branch node with a left branch child and a right terminal child.
pub const BRANCH_NODE_LBRT_DOMAIN: Fr = Fr::from_raw([8, 0, 0, 0]);

/// The hashing domain for a branch node with two branch children.
pub const BRANCH_NODE_LBRB_DOMAIN: Fr = Fr::from_raw([9, 0, 0, 0]);
