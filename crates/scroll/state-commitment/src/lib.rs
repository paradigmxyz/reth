//! The implementation of scrolls binary Merkle Patricia Trie used a cryptographic state commitment.

mod account;
pub use account::ScrollTrieAccount;

mod commitment;
pub use commitment::BinaryMerklePatriciaTrie;

mod root;
pub use root::*;

mod key;
pub use key::PoseidonKeyHasher;

mod value;
pub use value::PoseidonValueHasher;

/// test utils for the state commitment
#[cfg(feature = "test-utils")]
pub mod test_utils;

#[cfg(all(test, feature = "scroll"))]
mod test;
