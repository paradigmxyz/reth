//! Experimental partitioned binary state commitment defined by EIP-8297.
//!
//! The generic [`BinaryTree`] accepts arbitrary prefix-free byte keys and 32-byte values,
//! including zero. [`StateTree`] implements Ethereum's separate zero-means-delete rule and
//! account, storage, delegation and content-addressed code mapping.
//!
//! BLAKE3 matches the reference implementation; the EIP's hash choice is not final. A tree's
//! hasher is fixed by its type, and must be agreed upon by all participants in a chain.

mod hash;
pub use hash::{Blake3Hasher, TreeHasher};

mod tree;
pub use tree::{BinaryTree, TreeError, MAX_KEY_LENGTH};

mod proof;
pub use proof::{Proof, ProofError, ProofStep, ProofTerminal};

/// Ethereum state key derivation and value encoding.
pub mod key;

mod state;
pub use state::{AccountData, StateError, StateTree};

/// Canonical migration artifacts and independent MPT/PBT verification.
#[cfg(feature = "migration")]
pub mod migration;

/// Translation from Reth's authoritative block execution output.
#[cfg(feature = "execution")]
pub mod execution;

/// Block-level access-list replay into the partitioned tree.
#[cfg(feature = "bal")]
pub mod replay;

/// Incremental, content-addressed durable trie checkpoints.
#[cfg(feature = "persistence")]
pub mod persistence;

/// Conversion of retained MPT state using exact account and storage preimages.
#[cfg(feature = "conversion")]
pub mod conversion;
