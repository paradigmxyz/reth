//! Batched keccak256 used by level-batched trie hashing.

use alloy_primitives::{keccak256, B256};

/// Hashes every input with keccak256, writing the hash of `inputs[i]` to `out[i]`.
///
/// # Panics
///
/// Panics if `inputs` and `out` have different lengths.
#[inline]
pub fn keccak256_batch(inputs: &[&[u8]], out: &mut [B256]) {
    assert_eq!(inputs.len(), out.len(), "keccak256_batch: inputs and outputs differ in length");
    for (input, out) in inputs.iter().zip(out.iter_mut()) {
        *out = keccak256(input);
    }
}
