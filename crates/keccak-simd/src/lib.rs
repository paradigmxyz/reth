//! SIMD-accelerated batched Keccak-256 hashing.
//!
//! On x86-64 with AVX2, hashes 4 inputs simultaneously using XKCP's
//! KeccakP-1600-times4 permutation. Falls back to scalar hashing on
//! other architectures or when AVX2 is unavailable at runtime.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloc::{vec, vec::Vec};
use alloy_primitives::{keccak256, B256};

#[cfg(all(feature = "std", target_arch = "x86_64"))]
mod avx2 {
    unsafe extern "C" {
        /// Hash 4 inputs of `inlen` bytes each, producing 4 × 32-byte outputs.
        /// Inputs must be ≤ 135 bytes (single Keccak-256 block).
        pub(crate) fn keccak256_4x(
            in0: *const u8,
            in1: *const u8,
            in2: *const u8,
            in3: *const u8,
            inlen: u32,
            out0: *mut u8,
            out1: *mut u8,
            out2: *mut u8,
            out3: *mut u8,
        );
    }
}

/// Hash exactly 4 inputs of the same length, returning 4 digests.
///
/// Uses AVX2 4-way parallelism on x86-64 when available.
/// Falls back to scalar hashing otherwise.
///
/// # Panics
/// Panics if any input length exceeds 135 bytes (Keccak-256 rate - 1).
#[inline]
fn keccak256_4x(inputs: [&[u8]; 4]) -> [B256; 4] {
    let len = inputs[0].len();
    debug_assert!(inputs.iter().all(|i| i.len() == len));
    assert!(len <= 135, "input length must be <= 135 bytes");

    #[cfg(all(feature = "std", target_arch = "x86_64"))]
    {
        if std::is_x86_feature_detected!("avx2") {
            let mut out = [B256::ZERO; 4];
            // SAFETY: we verified AVX2 is available and all inputs have
            // the same length ≤ 135 bytes. The C function reads exactly
            // `inlen` bytes from each input pointer and writes exactly
            // 32 bytes to each output pointer.
            unsafe {
                avx2::keccak256_4x(
                    inputs[0].as_ptr(),
                    inputs[1].as_ptr(),
                    inputs[2].as_ptr(),
                    inputs[3].as_ptr(),
                    len as u32,
                    out[0].as_mut_ptr(),
                    out[1].as_mut_ptr(),
                    out[2].as_mut_ptr(),
                    out[3].as_mut_ptr(),
                );
            }
            return out;
        }
    }

    // Scalar fallback
    [keccak256(inputs[0]), keccak256(inputs[1]), keccak256(inputs[2]), keccak256(inputs[3])]
}

/// Batch-hash multiple 32-byte inputs, writing results into `outputs`.
///
/// Processes inputs in groups of 4 using AVX2 SIMD when available,
/// with scalar fallback for the remainder.
///
/// # Panics
/// Panics if `inputs.len() != outputs.len()`.
pub fn keccak256_batch_32(inputs: &[[u8; 32]], outputs: &mut [B256]) {
    assert_eq!(inputs.len(), outputs.len());

    let chunks = inputs.chunks_exact(4);
    let remainder = chunks.remainder();
    let mut out_idx = 0;

    for chunk in chunks {
        let results = keccak256_4x([&chunk[0], &chunk[1], &chunk[2], &chunk[3]]);
        outputs[out_idx] = results[0];
        outputs[out_idx + 1] = results[1];
        outputs[out_idx + 2] = results[2];
        outputs[out_idx + 3] = results[3];
        out_idx += 4;
    }

    for input in remainder {
        outputs[out_idx] = keccak256(input);
        out_idx += 1;
    }
}

/// Batch-hash multiple 20-byte inputs, writing results into `outputs`.
///
/// Processes inputs in groups of 4 using AVX2 SIMD when available,
/// with scalar fallback for the remainder.
///
/// # Panics
/// Panics if `inputs.len() != outputs.len()`.
pub fn keccak256_batch_20(inputs: &[[u8; 20]], outputs: &mut [B256]) {
    assert_eq!(inputs.len(), outputs.len());

    let chunks = inputs.chunks_exact(4);
    let remainder = chunks.remainder();
    let mut out_idx = 0;

    for chunk in chunks {
        let results = keccak256_4x([&chunk[0], &chunk[1], &chunk[2], &chunk[3]]);
        outputs[out_idx] = results[0];
        outputs[out_idx + 1] = results[1];
        outputs[out_idx + 2] = results[2];
        outputs[out_idx + 3] = results[3];
        out_idx += 4;
    }

    for input in remainder {
        outputs[out_idx] = keccak256(input);
        out_idx += 1;
    }
}

/// Convenience: batch-hash 32-byte inputs, returning a `Vec` of digests.
pub fn keccak256_batch_32_vec(inputs: &[[u8; 32]]) -> Vec<B256> {
    let mut outputs = vec![B256::ZERO; inputs.len()];
    keccak256_batch_32(inputs, &mut outputs);
    outputs
}

/// Convenience: batch-hash 20-byte inputs, returning a `Vec` of digests.
pub fn keccak256_batch_20_vec(inputs: &[[u8; 20]]) -> Vec<B256> {
    let mut outputs = vec![B256::ZERO; inputs.len()];
    keccak256_batch_20(inputs, &mut outputs);
    outputs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_batch_32_matches_scalar() {
        let inputs: Vec<[u8; 32]> = (0u8..13).map(|i| [i; 32]).collect();
        let expected: Vec<B256> = inputs.iter().map(keccak256).collect();
        let mut outputs = vec![B256::ZERO; inputs.len()];
        keccak256_batch_32(&inputs, &mut outputs);
        assert_eq!(outputs, expected);
    }

    #[test]
    fn test_batch_20_matches_scalar() {
        let inputs: Vec<[u8; 20]> = (0u8..11).map(|i| [i; 20]).collect();
        let expected: Vec<B256> = inputs.iter().map(keccak256).collect();
        let mut outputs = vec![B256::ZERO; inputs.len()];
        keccak256_batch_20(&inputs, &mut outputs);
        assert_eq!(outputs, expected);
    }

    #[test]
    fn test_batch_empty() {
        let inputs: &[[u8; 32]] = &[];
        let mut outputs: Vec<B256> = vec![];
        keccak256_batch_32(inputs, &mut outputs);
        assert!(outputs.is_empty());
    }

    #[test]
    fn test_batch_single() {
        let input = [0x42u8; 32];
        let expected = keccak256(input);
        let mut output = [B256::ZERO; 1];
        keccak256_batch_32(&[input], &mut output);
        assert_eq!(output[0], expected);
    }

    #[test]
    fn test_batch_exact_4() {
        let inputs: [[u8; 32]; 4] = [[1; 32], [2; 32], [3; 32], [4; 32]];
        let expected: Vec<B256> = inputs.iter().map(keccak256).collect();
        let mut outputs = [B256::ZERO; 4];
        keccak256_batch_32(&inputs, &mut outputs);
        assert_eq!(outputs.as_slice(), expected.as_slice());
    }

    #[test]
    fn test_known_address_hash() {
        let zero_addr = [0u8; 20];
        let expected = keccak256(zero_addr);
        let results = keccak256_batch_20_vec(&[zero_addr]);
        assert_eq!(results[0], expected);
    }

    #[test]
    fn test_known_slot_hash() {
        let zero_slot = [0u8; 32];
        let expected = keccak256(zero_slot);
        let results = keccak256_batch_32_vec(&[zero_slot]);
        assert_eq!(results[0], expected);
    }

    #[test]
    fn test_vec_convenience() {
        let inputs: Vec<[u8; 20]> = (0u8..8).map(|i| [i; 20]).collect();
        let results = keccak256_batch_20_vec(&inputs);
        for (input, result) in inputs.iter().zip(results.iter()) {
            assert_eq!(*result, keccak256(input));
        }
    }
}
