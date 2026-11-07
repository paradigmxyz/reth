//! Multi-lane Keccak-256 that hashes up to [`LANES`] independent messages in one pass.
//!
//! A single Keccak-f\[1600\] permutation is latency bound, so running the permutations of
//! independent messages side by side in SIMD registers costs about as much as running one.

use alloy_primitives::{keccak256, B256};

#[cfg(all(feature = "std", target_arch = "x86_64"))]
use core::arch::x86_64::*;

/// Maximum number of messages [`keccak256_batch`] hashes in one pass.
pub const LANES: usize = 8;

/// Keccak-256 rate in bytes.
const RATE: usize = 136;

/// Keccak-256 rate in 64-bit state words.
#[cfg(all(feature = "std", target_arch = "x86_64"))]
const RATE_WORDS: usize = RATE / 8;

/// Number of 64-bit state words that make up the 32-byte digest.
#[cfg(all(feature = "std", target_arch = "x86_64"))]
const DIGEST_WORDS: usize = 4;

/// Keccak-f\[1600\] round constants.
#[cfg(all(feature = "std", target_arch = "x86_64"))]
const ROUND_CONSTANTS: [u64; 24] = [
    0x0000000000000001,
    0x0000000000008082,
    0x800000000000808a,
    0x8000000080008000,
    0x000000000000808b,
    0x0000000080000001,
    0x8000000080008081,
    0x8000000000008009,
    0x000000000000008a,
    0x0000000000000088,
    0x0000000080008009,
    0x000000008000000a,
    0x000000008000808b,
    0x800000000000008b,
    0x8000000000008089,
    0x8000000000008003,
    0x8000000000008002,
    0x8000000000000080,
    0x000000000000800a,
    0x800000008000000a,
    0x8000000080008081,
    0x8000000000008080,
    0x0000000080000001,
    0x8000000080008008,
];

/// Hashes each of `inputs` with Keccak-256 (the same function as [`keccak256`]) into the
/// matching slot of `out`.
///
/// Messages may have different lengths. With the `std` feature, uses AVX-512 or AVX2 kernels
/// when the CPU has them.
///
/// # Panics
///
/// If `inputs` and `out` differ in length, or hold more than [`LANES`] items.
pub fn keccak256_batch(inputs: &[&[u8]], out: &mut [B256]) {
    assert_eq!(inputs.len(), out.len());
    assert!(inputs.len() <= LANES, "at most {LANES} inputs, got {}", inputs.len());

    // Every lane runs as many permutations as the longest message needs, and a vector
    // permutation costs about 1.65 scalar ones, so batching only pays when the messages need
    // enough permutations in total.
    let blocks = |input: &&[u8]| input.len() / RATE + 1;
    let total_blocks: usize = inputs.iter().map(blocks).sum();
    let max_blocks = inputs.iter().map(blocks).max().unwrap_or(0);
    if total_blocks * 3 <= max_blocks * 5 {
        return keccak256_batch_scalar(inputs, out);
    }

    #[cfg(all(feature = "std", target_arch = "x86_64"))]
    {
        if std::is_x86_feature_detected!("avx512f") {
            // SAFETY: the CPU supports AVX-512F.
            return unsafe { keccak256_batch_avx512(inputs, out) };
        }
        if std::is_x86_feature_detected!("avx2") {
            // SAFETY: the CPU supports AVX2.
            return unsafe { keccak256_batch_avx2(inputs, out) };
        }
    }

    keccak256_batch_scalar(inputs, out);
}

/// Hashes one message after another with [`keccak256`].
fn keccak256_batch_scalar(inputs: &[&[u8]], out: &mut [B256]) {
    for (input, out) in inputs.iter().zip(out) {
        *out = keccak256(input);
    }
}

/// Hashes up to [`LANES`] messages with 8-lane AVX-512 permutations.
#[cfg(all(feature = "std", target_arch = "x86_64"))]
#[target_feature(enable = "avx512f")]
fn keccak256_batch_avx512(inputs: &[&[u8]], out: &mut [B256]) {
    let mut state = [_mm512_setzero_si512(); 25];
    absorb_lanes(inputs, out, |block, digest| {
        for (word, lanes) in state.iter_mut().zip(block) {
            // SAFETY: `lanes` holds 8 u64s, the 64 bytes an unaligned load reads.
            let lanes = unsafe { _mm512_loadu_si512(lanes.as_ptr().cast()) };
            *word = _mm512_xor_si512(*word, lanes);
        }
        keccak_f1600_avx512(&mut state);
        for (lanes, word) in digest.iter_mut().zip(&state) {
            // SAFETY: `lanes` holds 8 u64s, the 64 bytes an unaligned store writes.
            unsafe { _mm512_storeu_si512(lanes.as_mut_ptr().cast(), *word) };
        }
    });
}

/// Hashes up to [`LANES`] messages with two 4-lane AVX2 permutations, skipping the second
/// when at most 4 messages are given.
#[cfg(all(feature = "std", target_arch = "x86_64"))]
#[target_feature(enable = "avx2")]
fn keccak256_batch_avx2(inputs: &[&[u8]], out: &mut [B256]) {
    let halves = inputs.len().div_ceil(4);
    let mut states = [[_mm256_setzero_si256(); 25]; 2];
    absorb_lanes(inputs, out, |block, digest| {
        for (half, state) in states.iter_mut().enumerate().take(halves) {
            for (word, lanes) in state.iter_mut().zip(block) {
                // SAFETY: `lanes[4 * half..]` holds at least 4 u64s, the 32 bytes an unaligned
                // load reads.
                let lanes = unsafe { _mm256_loadu_si256(lanes[4 * half..].as_ptr().cast()) };
                *word = _mm256_xor_si256(*word, lanes);
            }
            keccak_f1600_avx2(state);
            for (lanes, word) in digest.iter_mut().zip(state.iter()) {
                // SAFETY: `lanes[4 * half..]` holds at least 4 u64s, the 32 bytes an unaligned
                // store writes.
                unsafe { _mm256_storeu_si256(lanes[4 * half..].as_mut_ptr().cast(), *word) };
            }
        }
    });
}

/// Feeds the padded blocks of `inputs` to `absorb_permute` and writes each digest to `out`.
///
/// `absorb_permute` XORs a block into the state of every lane, permutes it, and stores the
/// first [`DIGEST_WORDS`] state words. Blocks are transposed: `block[word][lane]`. A lane past
/// its final block, or without a message, gets zero words, which leave its state unchanged
/// before the permutation; its digest is taken right after its final block.
#[cfg(all(feature = "std", target_arch = "x86_64"))]
#[inline(always)]
fn absorb_lanes(
    inputs: &[&[u8]],
    out: &mut [B256],
    mut absorb_permute: impl FnMut(&[[u64; LANES]; RATE_WORDS], &mut [[u64; LANES]; DIGEST_WORDS]),
) {
    // Messages get at least one more block than their full blocks, for the padding.
    let blocks = inputs.iter().map(|input| input.len() / RATE + 1).max().unwrap_or(0);
    let mut digest = [[0u64; LANES]; DIGEST_WORDS];
    for block_index in 0..blocks {
        let mut block = [[0u64; LANES]; RATE_WORDS];
        let offset = block_index * RATE;
        for (lane, input) in inputs.iter().enumerate() {
            if let Some(full) = input.get(offset..offset + RATE) {
                let (words, _) = full.as_chunks::<8>();
                for (block_word, word) in block.iter_mut().zip(words) {
                    block_word[lane] = u64::from_le_bytes(*word);
                }
            } else if let Some(rest) = input.get(offset..) {
                let mut padded = [0u8; RATE];
                padded[..rest.len()].copy_from_slice(rest);
                padded[rest.len()] ^= 0x01;
                padded[RATE - 1] ^= 0x80;
                let (words, _) = padded.as_chunks::<8>();
                for (block_word, word) in block.iter_mut().zip(words) {
                    block_word[lane] = u64::from_le_bytes(*word);
                }
            }
        }

        absorb_permute(&block, &mut digest);

        for (lane, (input, out)) in inputs.iter().zip(out.iter_mut()).enumerate() {
            if input.len() / RATE == block_index {
                for (bytes, word) in out.0.as_chunks_mut::<8>().0.iter_mut().zip(&digest) {
                    *bytes = word[lane].to_le_bytes();
                }
            }
        }
    }
}

/// Expands to the 24 rounds of Keccak-f\[1600\] over `$a`, an array of 25 vectors that each hold
/// one state word of every lane.
///
/// `$xor` and `$xor5` XOR 2 and 5 vectors, `$chi` computes `a ^ (!b & c)`, `$rol` is a macro
/// rotating every lane left by a literal, and `$splat` broadcasts a u64 to every lane.
#[cfg(all(feature = "std", target_arch = "x86_64"))]
macro_rules! keccak_f1600 {
    ($a:ident, $xor:path, $xor5:path, $chi:path, $rol:ident, $splat:path) => {
        for round_constant in ROUND_CONSTANTS {
            // Theta.
            let mut c = [$a[0]; 5];
            for x in 0..5 {
                c[x] = $xor5($a[x], $a[x + 5], $a[x + 10], $a[x + 15], $a[x + 20]);
            }
            let mut d = c;
            for x in 0..5 {
                d[x] = $xor(c[(x + 4) % 5], $rol!(c[(x + 1) % 5], 1));
            }

            // Rho and pi: word `x + 5y` moves to `y + 5((2x + 3y) % 5)`.
            let b = [
                $xor($a[0], d[0]),
                $rol!($xor($a[6], d[1]), 44),
                $rol!($xor($a[12], d[2]), 43),
                $rol!($xor($a[18], d[3]), 21),
                $rol!($xor($a[24], d[4]), 14),
                $rol!($xor($a[3], d[3]), 28),
                $rol!($xor($a[9], d[4]), 20),
                $rol!($xor($a[10], d[0]), 3),
                $rol!($xor($a[16], d[1]), 45),
                $rol!($xor($a[22], d[2]), 61),
                $rol!($xor($a[1], d[1]), 1),
                $rol!($xor($a[7], d[2]), 6),
                $rol!($xor($a[13], d[3]), 25),
                $rol!($xor($a[19], d[4]), 8),
                $rol!($xor($a[20], d[0]), 18),
                $rol!($xor($a[4], d[4]), 27),
                $rol!($xor($a[5], d[0]), 36),
                $rol!($xor($a[11], d[1]), 10),
                $rol!($xor($a[17], d[2]), 15),
                $rol!($xor($a[23], d[3]), 56),
                $rol!($xor($a[2], d[2]), 62),
                $rol!($xor($a[8], d[3]), 55),
                $rol!($xor($a[14], d[4]), 39),
                $rol!($xor($a[15], d[0]), 41),
                $rol!($xor($a[21], d[1]), 2),
            ];

            // Chi.
            for y in (0..25).step_by(5) {
                for x in 0..5 {
                    $a[y + x] = $chi(b[y + x], b[y + (x + 1) % 5], b[y + (x + 2) % 5]);
                }
            }

            // Iota.
            $a[0] = $xor($a[0], $splat(round_constant.cast_signed()));
        }
    };
}

/// Rotates every 64-bit lane of an `__m512i` left by a literal.
#[cfg(all(feature = "std", target_arch = "x86_64"))]
macro_rules! rol_avx512 {
    ($x:expr, $n:literal) => {
        _mm512_rol_epi64::<$n>($x)
    };
}

/// Rotates every 64-bit lane of an `__m256i` left by a literal.
#[cfg(all(feature = "std", target_arch = "x86_64"))]
macro_rules! rol_avx2 {
    ($x:expr, $n:literal) => {
        _mm256_or_si256(_mm256_slli_epi64::<$n>($x), _mm256_srli_epi64::<{ 64 - $n }>($x))
    };
}

/// Keccak-f\[1600\] over 8 lanes.
#[cfg(all(feature = "std", target_arch = "x86_64"))]
#[target_feature(enable = "avx512f")]
fn keccak_f1600_avx512(state: &mut [__m512i; 25]) {
    // A local copy keeps the state in registers across rounds.
    let mut a = *state;
    keccak_f1600!(a, _mm512_xor_si512, xor5_avx512, chi_avx512, rol_avx512, _mm512_set1_epi64);
    *state = a;
}

/// XORs 5 vectors with two ternary-logic instructions.
#[cfg(all(feature = "std", target_arch = "x86_64"))]
#[target_feature(enable = "avx512f")]
fn xor5_avx512(a: __m512i, b: __m512i, c: __m512i, d: __m512i, e: __m512i) -> __m512i {
    _mm512_ternarylogic_epi64::<0x96>(_mm512_ternarylogic_epi64::<0x96>(a, b, c), d, e)
}

/// Computes `a ^ (!b & c)` with one ternary-logic instruction.
#[cfg(all(feature = "std", target_arch = "x86_64"))]
#[target_feature(enable = "avx512f")]
fn chi_avx512(a: __m512i, b: __m512i, c: __m512i) -> __m512i {
    _mm512_ternarylogic_epi64::<0xD2>(a, b, c)
}

/// Keccak-f\[1600\] over 4 lanes.
#[cfg(all(feature = "std", target_arch = "x86_64"))]
#[target_feature(enable = "avx2")]
fn keccak_f1600_avx2(state: &mut [__m256i; 25]) {
    // A local copy keeps the state in registers across rounds.
    let mut a = *state;
    keccak_f1600!(a, _mm256_xor_si256, xor5_avx2, chi_avx2, rol_avx2, _mm256_set1_epi64x);
    *state = a;
}

/// XORs 5 vectors.
#[cfg(all(feature = "std", target_arch = "x86_64"))]
#[target_feature(enable = "avx2")]
fn xor5_avx2(a: __m256i, b: __m256i, c: __m256i, d: __m256i, e: __m256i) -> __m256i {
    _mm256_xor_si256(_mm256_xor_si256(_mm256_xor_si256(a, b), _mm256_xor_si256(c, d)), e)
}

/// Computes `a ^ (!b & c)`.
#[cfg(all(feature = "std", target_arch = "x86_64"))]
#[target_feature(enable = "avx2")]
fn chi_avx2(a: __m256i, b: __m256i, c: __m256i) -> __m256i {
    _mm256_xor_si256(a, _mm256_andnot_si256(b, c))
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;

    /// Deterministic pseudo-random numbers for the tests.
    struct XorShift(u64);

    impl XorShift {
        const fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        const fn below(&mut self, bound: usize) -> usize {
            (self.next() % bound as u64) as usize
        }

        fn bytes(&mut self, len: usize) -> Vec<u8> {
            (0..len).map(|_| self.next() as u8).collect()
        }
    }

    type Kernel = fn(&[&[u8]], &mut [B256]);

    /// Every kernel the CPU supports.
    fn kernels() -> Vec<(&'static str, Kernel)> {
        let mut kernels =
            vec![("dispatch", keccak256_batch as Kernel), ("scalar", keccak256_batch_scalar)];
        #[cfg(all(feature = "std", target_arch = "x86_64"))]
        {
            if std::is_x86_feature_detected!("avx512f") {
                // SAFETY: the CPU supports AVX-512F.
                kernels
                    .push(("avx512", |inputs, out| unsafe { keccak256_batch_avx512(inputs, out) }));
            }
            if std::is_x86_feature_detected!("avx2") {
                // SAFETY: the CPU supports AVX2.
                kernels.push(("avx2", |inputs, out| unsafe { keccak256_batch_avx2(inputs, out) }));
            }
        }
        kernels
    }

    fn assert_batch((name, hash): (&str, Kernel), inputs: &[&[u8]]) {
        let mut out = [B256::ZERO; LANES];
        hash(inputs, &mut out[..inputs.len()]);
        for (lane, (input, digest)) in inputs.iter().zip(&out).enumerate() {
            assert_eq!(
                *digest,
                keccak256(input),
                "kernel {name}, lane {lane} of {}, length {}",
                inputs.len(),
                input.len()
            );
        }
    }

    #[test]
    fn every_length_in_every_lane() {
        let mut rng = XorShift(0x9e37_79b9_7f4a_7c15);
        let data = rng.bytes(701);
        let filler = rng.bytes(300);

        for kernel in kernels() {
            for len in 0..=700 {
                for lane in 0..LANES {
                    let mut inputs = [&filler[..]; LANES];
                    for (other, input) in inputs.iter_mut().enumerate() {
                        *input = &filler[..(other * 37) % filler.len()];
                    }
                    inputs[lane] = &data[..len];
                    assert_batch(kernel, &inputs);
                }
            }
        }
    }

    #[test]
    fn random_mixed_batches() {
        let mut rng = XorShift(0x2545_f491_4f6c_dd1d);
        let data = rng.bytes(1100);

        for kernel in kernels() {
            for _ in 0..2000 {
                let size = rng.below(LANES + 1);
                let inputs: Vec<&[u8]> = (0..size)
                    .map(|_| {
                        let start = rng.below(100);
                        let len = rng.below(1000);
                        &data[start..start + len]
                    })
                    .collect();
                assert_batch(kernel, &inputs);
            }
        }
    }

    #[test]
    fn every_batch_size() {
        let data: Vec<u8> = (0..=255).cycle().take(600).collect();
        for kernel in kernels() {
            for size in 0..=LANES {
                let inputs: Vec<&[u8]> = (0..size).map(|lane| &data[..33 + lane * 70]).collect();
                assert_batch(kernel, &inputs);
            }
        }
    }

    #[test]
    #[should_panic(expected = "at most 8 inputs, got 9")]
    fn too_many_inputs() {
        let inputs = [&[][..]; LANES + 1];
        let mut out = [B256::ZERO; LANES + 1];
        keccak256_batch(&inputs, &mut out);
    }
}
