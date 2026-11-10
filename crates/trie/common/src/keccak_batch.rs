//! Batched `keccak256` for many independent short inputs.
//!
//! Hashing the keys of a state update (addresses and storage slots) or many trie leaves is a
//! stream of independent `keccak256` calls on inputs shorter than one Keccak block. On CPUs with
//! AVX-512 this runs eight permutations at once, one per 64-bit lane, which is several times the
//! throughput of hashing them one at a time. Other CPUs fall back to [`keccak256`].

use alloc::vec::Vec;
use alloy_primitives::{keccak256, B256};

/// Largest input, in bytes, that fits in a single Keccak-256 block with its padding.
pub const KECCAK256_BATCH_MAX_INPUT: usize = 135;

/// Smallest batch for which the vectorized path is used. Smaller batches are hashed one by one.
#[cfg(all(feature = "std", target_arch = "x86_64"))]
const MIN_VECTOR_BATCH: usize = 3;

/// Returns `keccak256` of every input, in order.
///
/// Inputs of at most [`KECCAK256_BATCH_MAX_INPUT`] bytes are hashed eight at a time when the CPU
/// supports AVX-512; longer inputs, and every input on other CPUs, use [`keccak256`].
pub fn keccak256_batch<T: AsRef<[u8]>>(inputs: &[T]) -> Vec<B256> {
    let mut out = Vec::with_capacity(inputs.len());
    keccak256_batch_into(inputs, &mut out);
    out
}

/// Appends `keccak256` of every input to `out`, in order. See [`keccak256_batch`].
pub fn keccak256_batch_into<T: AsRef<[u8]>>(inputs: &[T], out: &mut Vec<B256>) {
    #[cfg(all(feature = "std", target_arch = "x86_64"))]
    if inputs.len() >= MIN_VECTOR_BATCH && x86::is_supported() {
        // SAFETY: AVX-512F support was checked at runtime.
        unsafe { x86::keccak256_batch_into(inputs, out) };
        return;
    }
    out.extend(inputs.iter().map(|input| keccak256(input.as_ref())));
}

#[cfg(all(feature = "std", target_arch = "x86_64"))]
mod x86 {
    use super::{KECCAK256_BATCH_MAX_INPUT, MIN_VECTOR_BATCH};
    use alloc::vec::Vec;
    use alloy_primitives::{keccak256, B256};
    use core::arch::x86_64::*;

    /// Keccak-256 rate in 64-bit words.
    const RATE_WORDS: usize = 17;

    const RC: [u64; 24] = [
        0x1,
        0x8082,
        0x800000000000808a,
        0x8000000080008000,
        0x808b,
        0x80000001,
        0x8000000080008081,
        0x8000000000008009,
        0x8a,
        0x88,
        0x80008009,
        0x8000000a,
        0x8000808b,
        0x800000000000008b,
        0x8000000000008089,
        0x8000000000008003,
        0x8000000000008002,
        0x8000000000000080,
        0x800a,
        0x800000008000000a,
        0x8000000080008081,
        0x8000000000008080,
        0x80000001,
        0x8000000080008008,
    ];

    /// Whether the CPU supports the AVX-512 path. `std` detects it once and caches it.
    pub(super) fn is_supported() -> bool {
        std::is_x86_feature_detected!("avx512f")
    }

    /// # Safety
    ///
    /// The CPU must support AVX-512F.
    #[target_feature(enable = "avx512f")]
    pub(super) unsafe fn keccak256_batch_into<T: AsRef<[u8]>>(inputs: &[T], out: &mut Vec<B256>) {
        let start = out.len();
        out.resize(start + inputs.len(), B256::ZERO);
        let mut lanes: [&[u8]; 8] = [&[]; 8];
        let mut slots = [0usize; 8];
        let mut n = 0;
        for (i, input) in inputs.iter().enumerate() {
            let input = input.as_ref();
            if input.len() > KECCAK256_BATCH_MAX_INPUT {
                out[start + i] = keccak256(input);
                continue;
            }
            lanes[n] = input;
            slots[n] = start + i;
            n += 1;
            if n == 8 {
                hash_lanes(&lanes, &slots, n, out);
                n = 0;
            }
        }
        if n >= MIN_VECTOR_BATCH {
            hash_lanes(&lanes, &slots, n, out);
        } else {
            for l in 0..n {
                out[slots[l]] = keccak256(lanes[l]);
            }
        }
    }

    /// Hashes the first `n` lanes, each a single-block input, into `out[slots[l]]`.
    #[target_feature(enable = "avx512f")]
    fn hash_lanes(lanes: &[&[u8]; 8], slots: &[usize; 8], n: usize, out: &mut [B256]) {
        // Pad each lane into one 136-byte block: Keccak padding is 0x01 after the message and
        // 0x80 in the last byte of the block.
        let mut blocks = [[0u8; RATE_WORDS * 8]; 8];
        for l in 0..n {
            let msg = lanes[l];
            blocks[l][..msg.len()].copy_from_slice(msg);
            blocks[l][msg.len()] ^= 0x01;
            blocks[l][RATE_WORDS * 8 - 1] ^= 0x80;
        }
        let mut state = [_mm512_setzero_si512(); 25];
        for (w, word) in state.iter_mut().take(RATE_WORDS).enumerate() {
            let mut column = [0u64; 8];
            for (l, block) in blocks.iter().enumerate() {
                column[l] = u64::from_le_bytes(block[w * 8..w * 8 + 8].try_into().unwrap());
            }
            // SAFETY: `column` is 64 bytes, and the load is unaligned.
            *word = unsafe { _mm512_loadu_si512(column.as_ptr().cast()) };
        }
        keccak_f1600_x8(&mut state);
        let mut words = [[0u64; 8]; 4];
        for (w, word) in words.iter_mut().enumerate() {
            // SAFETY: `word` is 64 bytes, and the store is unaligned.
            unsafe { _mm512_storeu_si512(word.as_mut_ptr().cast(), state[w]) };
        }
        for l in 0..n {
            let hash = &mut out[slots[l]].0;
            for (w, word) in words.iter().enumerate() {
                hash[w * 8..w * 8 + 8].copy_from_slice(&word[l].to_le_bytes());
            }
        }
    }

    /// Keccak-f[1600] on eight independent states, one state per 64-bit lane.
    #[target_feature(enable = "avx512f")]
    fn keccak_f1600_x8(s: &mut [__m512i; 25]) {
        let [mut a0, mut a1, mut a2, mut a3, mut a4, mut a5, mut a6, mut a7, mut a8, mut a9, mut a10, mut a11, mut a12, mut a13, mut a14, mut a15, mut a16, mut a17, mut a18, mut a19, mut a20, mut a21, mut a22, mut a23, mut a24] =
            *s;
        for rc in RC {
            let c0 = _mm512_ternarylogic_epi64::<0x96>(
                _mm512_ternarylogic_epi64::<0x96>(a0, a5, a10),
                a15,
                a20,
            );
            let c1 = _mm512_ternarylogic_epi64::<0x96>(
                _mm512_ternarylogic_epi64::<0x96>(a1, a6, a11),
                a16,
                a21,
            );
            let c2 = _mm512_ternarylogic_epi64::<0x96>(
                _mm512_ternarylogic_epi64::<0x96>(a2, a7, a12),
                a17,
                a22,
            );
            let c3 = _mm512_ternarylogic_epi64::<0x96>(
                _mm512_ternarylogic_epi64::<0x96>(a3, a8, a13),
                a18,
                a23,
            );
            let c4 = _mm512_ternarylogic_epi64::<0x96>(
                _mm512_ternarylogic_epi64::<0x96>(a4, a9, a14),
                a19,
                a24,
            );
            let d0 = _mm512_xor_si512(c4, _mm512_rol_epi64::<1>(c1));
            let d1 = _mm512_xor_si512(c0, _mm512_rol_epi64::<1>(c2));
            let d2 = _mm512_xor_si512(c1, _mm512_rol_epi64::<1>(c3));
            let d3 = _mm512_xor_si512(c2, _mm512_rol_epi64::<1>(c4));
            let d4 = _mm512_xor_si512(c3, _mm512_rol_epi64::<1>(c0));
            a0 = _mm512_xor_si512(a0, d0);
            a5 = _mm512_xor_si512(a5, d0);
            a10 = _mm512_xor_si512(a10, d0);
            a15 = _mm512_xor_si512(a15, d0);
            a20 = _mm512_xor_si512(a20, d0);
            a1 = _mm512_xor_si512(a1, d1);
            a6 = _mm512_xor_si512(a6, d1);
            a11 = _mm512_xor_si512(a11, d1);
            a16 = _mm512_xor_si512(a16, d1);
            a21 = _mm512_xor_si512(a21, d1);
            a2 = _mm512_xor_si512(a2, d2);
            a7 = _mm512_xor_si512(a7, d2);
            a12 = _mm512_xor_si512(a12, d2);
            a17 = _mm512_xor_si512(a17, d2);
            a22 = _mm512_xor_si512(a22, d2);
            a3 = _mm512_xor_si512(a3, d3);
            a8 = _mm512_xor_si512(a8, d3);
            a13 = _mm512_xor_si512(a13, d3);
            a18 = _mm512_xor_si512(a18, d3);
            a23 = _mm512_xor_si512(a23, d3);
            a4 = _mm512_xor_si512(a4, d4);
            a9 = _mm512_xor_si512(a9, d4);
            a14 = _mm512_xor_si512(a14, d4);
            a19 = _mm512_xor_si512(a19, d4);
            a24 = _mm512_xor_si512(a24, d4);
            let b0 = a0;
            let b1 = _mm512_rol_epi64::<44>(a6);
            let b2 = _mm512_rol_epi64::<43>(a12);
            let b3 = _mm512_rol_epi64::<21>(a18);
            let b4 = _mm512_rol_epi64::<14>(a24);
            let b5 = _mm512_rol_epi64::<28>(a3);
            let b6 = _mm512_rol_epi64::<20>(a9);
            let b7 = _mm512_rol_epi64::<3>(a10);
            let b8 = _mm512_rol_epi64::<45>(a16);
            let b9 = _mm512_rol_epi64::<61>(a22);
            let b10 = _mm512_rol_epi64::<1>(a1);
            let b11 = _mm512_rol_epi64::<6>(a7);
            let b12 = _mm512_rol_epi64::<25>(a13);
            let b13 = _mm512_rol_epi64::<8>(a19);
            let b14 = _mm512_rol_epi64::<18>(a20);
            let b15 = _mm512_rol_epi64::<27>(a4);
            let b16 = _mm512_rol_epi64::<36>(a5);
            let b17 = _mm512_rol_epi64::<10>(a11);
            let b18 = _mm512_rol_epi64::<15>(a17);
            let b19 = _mm512_rol_epi64::<56>(a23);
            let b20 = _mm512_rol_epi64::<62>(a2);
            let b21 = _mm512_rol_epi64::<55>(a8);
            let b22 = _mm512_rol_epi64::<39>(a14);
            let b23 = _mm512_rol_epi64::<41>(a15);
            let b24 = _mm512_rol_epi64::<2>(a21);
            a0 = _mm512_ternarylogic_epi64::<0xD2>(b0, b1, b2);
            a5 = _mm512_ternarylogic_epi64::<0xD2>(b5, b6, b7);
            a10 = _mm512_ternarylogic_epi64::<0xD2>(b10, b11, b12);
            a15 = _mm512_ternarylogic_epi64::<0xD2>(b15, b16, b17);
            a20 = _mm512_ternarylogic_epi64::<0xD2>(b20, b21, b22);
            a1 = _mm512_ternarylogic_epi64::<0xD2>(b1, b2, b3);
            a6 = _mm512_ternarylogic_epi64::<0xD2>(b6, b7, b8);
            a11 = _mm512_ternarylogic_epi64::<0xD2>(b11, b12, b13);
            a16 = _mm512_ternarylogic_epi64::<0xD2>(b16, b17, b18);
            a21 = _mm512_ternarylogic_epi64::<0xD2>(b21, b22, b23);
            a2 = _mm512_ternarylogic_epi64::<0xD2>(b2, b3, b4);
            a7 = _mm512_ternarylogic_epi64::<0xD2>(b7, b8, b9);
            a12 = _mm512_ternarylogic_epi64::<0xD2>(b12, b13, b14);
            a17 = _mm512_ternarylogic_epi64::<0xD2>(b17, b18, b19);
            a22 = _mm512_ternarylogic_epi64::<0xD2>(b22, b23, b24);
            a3 = _mm512_ternarylogic_epi64::<0xD2>(b3, b4, b0);
            a8 = _mm512_ternarylogic_epi64::<0xD2>(b8, b9, b5);
            a13 = _mm512_ternarylogic_epi64::<0xD2>(b13, b14, b10);
            a18 = _mm512_ternarylogic_epi64::<0xD2>(b18, b19, b15);
            a23 = _mm512_ternarylogic_epi64::<0xD2>(b23, b24, b20);
            a4 = _mm512_ternarylogic_epi64::<0xD2>(b4, b0, b1);
            a9 = _mm512_ternarylogic_epi64::<0xD2>(b9, b5, b6);
            a14 = _mm512_ternarylogic_epi64::<0xD2>(b14, b10, b11);
            a19 = _mm512_ternarylogic_epi64::<0xD2>(b19, b15, b16);
            a24 = _mm512_ternarylogic_epi64::<0xD2>(b24, b20, b21);
            a0 = _mm512_xor_si512(a0, _mm512_set1_epi64(rc as i64));
        }
        *s = [
            a0, a1, a2, a3, a4, a5, a6, a7, a8, a9, a10, a11, a12, a13, a14, a15, a16, a17, a18,
            a19, a20, a21, a22, a23, a24,
        ];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_keccak256() {
        // Lengths around the single-block limit, in batch sizes that cover the scalar tail, a
        // partial vector batch and full batches.
        for count in [0usize, 1, 2, 3, 7, 8, 9, 17, 64] {
            for len in [0usize, 1, 20, 32, 64, 100, 134, 135, 136, 200] {
                let inputs: Vec<Vec<u8>> = (0..count)
                    .map(|i| (0..len).map(|j| (i * 31 + j * 7) as u8).collect())
                    .collect();
                let expected: Vec<B256> = inputs.iter().map(keccak256).collect();
                assert_eq!(keccak256_batch(&inputs), expected, "count {count} len {len}");
            }
        }
        // Mixed lengths in one batch, including inputs that need the scalar fallback.
        let mixed: Vec<Vec<u8>> = (0..37).map(|i| vec![i as u8; (i * 13) % 180]).collect();
        let expected: Vec<B256> = mixed.iter().map(keccak256).collect();
        assert_eq!(keccak256_batch(&mixed), expected);
    }
}
