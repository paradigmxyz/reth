//! Reth adapters for the external `pbt` commitment library.
//!
//! The tree, Ethereum embedding, proofs, artifacts, replay and native node database live in
//! `Rimeeeeee/pbtree`. This crate retains Reth execution/database adapters and atomic checkpoint
//! publication. Re-exports preserve existing integration call sites.

pub use pbt::{
    key, AccountData, BinaryTree, Blake3Hasher, Proof, ProofError, ProofStep, ProofTerminal,
    StateError, StateTree, TreeError, TreeHasher, MAX_KEY_LENGTH,
};

/// Canonical migration artifacts and independent MPT/PBT verification.
#[cfg(feature = "migration")]
pub use pbt::migration;

/// Block-level access-list replay into the partitioned tree.
#[cfg(feature = "bal")]
pub use pbt::replay;

/// Translation from Reth's authoritative block execution output.
#[cfg(feature = "execution")]
pub mod execution;

/// Incremental durable checkpoint publication using Reth filesystem helpers.
#[cfg(feature = "persistence")]
pub mod persistence;

/// Conversion of retained Reth MPT state using exact account and storage preimages.
#[cfg(feature = "conversion")]
pub mod conversion;
