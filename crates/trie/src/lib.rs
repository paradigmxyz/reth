#![warn(missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! The implementation of Merkle Patricia Trie, a cryptographically
//! authenticated radix trie that is used to store key-value bindings.
//! <https://ethereum.org/en/developers/docs/data-structures-and-encoding/patricia-merkle-trie/>

/// Various branch nodes producde by the hash builder.
pub mod nodes;

/// The implementation of hash builder.
pub mod hash_builder;

/// The implementation of a container for storing intermediate changes to a trie.
/// The container indicates when the trie has been modified.
pub mod prefix_set;
