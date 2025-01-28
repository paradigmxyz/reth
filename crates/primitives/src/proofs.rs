//! Helper function for calculating Merkle proofs and hashes.
pub use alloy_trie::root::ordered_trie_root_with_encoder;

pub use alloy_consensus::proofs::calculate_receipt_root;

/// Calculate a transaction root.
///
/// `(rlp(index), encoded(tx))` pairs.
#[doc(inline)]
pub use alloy_consensus::proofs::calculate_transaction_root;

/// Calculates the root hash of the withdrawals.
#[doc(inline)]
pub use alloy_consensus::proofs::calculate_withdrawals_root;

/// Calculates the root hash for ommer/uncle headers.
#[doc(inline)]
pub use alloy_consensus::proofs::calculate_ommers_root;
