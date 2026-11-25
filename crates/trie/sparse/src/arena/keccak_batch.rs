//! Batched keccak256 used by level-batched trie hashing.

use alloy_primitives::{keccak256, B256};

/// Hashes every input with keccak256, writing the hash of `inputs[i]` to `out[i]`.
///
/// This is the single entry point for level-batched hashing, so that a multi-buffer (SIMD)
/// keccak implementation hashing several inputs per permutation can be swapped in. Currently
/// hashes one input at a time.
///
/// # Panics
///
/// Panics if `inputs` and `out` have different lengths.
#[inline]
pub fn keccak256_batch(inputs: &[&[u8]], out: &mut [B256]) {
    assert_eq!(inputs.len(), out.len(), "keccak256_batch: inputs and outputs differ in length");
    #[cfg(target_arch = "x86_64")]
    if inputs.len() >= 4 {
        // SAFETY: `B256` is `#[repr(transparent)]` over `[u8; 32]`, so the slices have the same
        // layout and length.
        let out = unsafe {
            core::slice::from_raw_parts_mut(out.as_mut_ptr().cast::<[u8; 32]>(), out.len())
        };
        mbkeccak::keccak256_batch(inputs, out);
        return
    }
    for (input, out) in inputs.iter().zip(out.iter_mut()) {
        *out = keccak256(input);
    }
}
