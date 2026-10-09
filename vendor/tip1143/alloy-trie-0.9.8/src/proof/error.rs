use alloy_primitives::{B256, Bytes};
use nybbles::Nibbles;

/// Error during proof verification.
#[derive(PartialEq, Eq, Debug, thiserror::Error)]
pub enum ProofVerificationError {
    /// State root does not match the expected.
    #[error("root mismatch. got: {got}. expected: {expected}")]
    RootMismatch {
        /// Computed state root.
        got: B256,
        /// State root provided to verify function.
        expected: B256,
    },
    /// The node value does not match at specified path.
    #[error("value mismatch at path {path:?}. got: {got:?}. expected: {expected:?}")]
    ValueMismatch {
        /// Path at which error occurred.
        path: Nibbles,
        /// Value in the proof.
        got: Option<Bytes>,
        /// Expected value.
        expected: Option<Bytes>,
    },
    /// Encountered unexpected empty root node.
    #[error("unexpected empty root node")]
    UnexpectedEmptyRoot,
    /// Error during RLP decoding of trie node.
    #[error(transparent)]
    Rlp(#[from] alloy_rlp::Error),
}
