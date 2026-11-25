//! Multi-buffer Keccak-256.
//!
//! Hashes many independent messages at once by interleaving their Keccak-f[1600] states across
//! SIMD lanes: 8 messages per AVX-512 permutation, 4 per AVX2 permutation. Lanes are refilled as
//! soon as their message finishes, so a batch of mixed-length inputs keeps every lane busy
//! without sorting or grouping; inputs are fed longest-first to keep the drain short.
//!
//! The output is bit-identical to `keccak256` as used by Ethereum (original Keccak padding,
//! `0x01 .. 0x80`, rate 136 bytes).
//!
//! ```
//! let a = b"hello";
//! let b = [0u8; 300];
//! let mut out = [[0u8; 32]; 2];
//! mbkeccak::keccak256_batch(&[&a[..], &b[..]], &mut out);
//! assert_eq!(out[0], mbkeccak::keccak256(a));
//! ```
#![cfg_attr(not(feature = "std"), no_std)]
#![deny(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

#[macro_use]
mod round;
mod sched;
mod scalar;
#[cfg(target_arch = "x86_64")]
mod x86;

use core::sync::atomic::{AtomicU8, Ordering};

/// Keccak-256 rate in bytes.
pub const RATE: usize = 136;

pub(crate) const RC: [u64; 24] = [
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

/// A Keccak-f[1600] implementation the batch hasher can run on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Backend {
    /// Portable scalar code, one message at a time.
    Scalar,
    /// x86_64 AVX2: 4 messages per permutation.
    Avx2,
    /// x86_64 AVX-512F + VL on 256-bit registers (ternary logic, native rotates): 4 messages per
    /// permutation.
    Avx512Vl4,
    /// x86_64 AVX-512F on 512-bit registers: 8 messages per permutation.
    Avx512,
}

impl Backend {
    /// Every backend, fastest first.
    pub const ALL: [Backend; 4] = [Backend::Avx512, Backend::Avx512Vl4, Backend::Avx2, Backend::Scalar];

    /// Number of messages hashed per permutation.
    pub const fn lanes(self) -> usize {
        match self {
            Backend::Scalar => 1,
            Backend::Avx2 | Backend::Avx512Vl4 => 4,
            Backend::Avx512 => 8,
        }
    }

    /// Whether this CPU (and OS) supports the backend.
    pub fn is_supported(self) -> bool {
        let f = features();
        match self {
            Backend::Scalar => true,
            Backend::Avx2 => f & F_AVX2 != 0,
            Backend::Avx512Vl4 | Backend::Avx512 => f & F_AVX512 != 0,
        }
    }
}

const F_INIT: u8 = 1;
const F_AVX2: u8 = 2;
const F_AVX512: u8 = 4;
static FEATURES: AtomicU8 = AtomicU8::new(0);

#[inline]
fn features() -> u8 {
    let f = FEATURES.load(Ordering::Relaxed);
    if f != 0 {
        return f;
    }
    let f = F_INIT | detect_features();
    FEATURES.store(f, Ordering::Relaxed);
    f
}

#[cfg(all(target_arch = "x86_64", feature = "std"))]
fn detect_features() -> u8 {
    let mut f = 0;
    if std::is_x86_feature_detected!("avx2") {
        f |= F_AVX2;
    }
    if std::is_x86_feature_detected!("avx512f")
        && std::is_x86_feature_detected!("avx512vl")
        && std::is_x86_feature_detected!("avx512bw")
    {
        f |= F_AVX512;
    }
    f
}

#[cfg(all(target_arch = "x86_64", not(feature = "std")))]
fn detect_features() -> u8 {
    x86::detect_raw()
}

#[cfg(not(target_arch = "x86_64"))]
fn detect_features() -> u8 {
    0
}

/// The backend [`keccak256_batch`] uses on this CPU.
pub fn best_backend() -> Backend {
    let f = features();
    if f & F_AVX512 != 0 {
        Backend::Avx512
    } else if f & F_AVX2 != 0 {
        Backend::Avx2
    } else {
        Backend::Scalar
    }
}

/// Error returned by [`keccak256_batch_with`] when the CPU lacks the requested backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unsupported(pub Backend);

/// Hashes every `inputs[i]` into `out[i]` with Keccak-256, using the fastest backend this CPU
/// supports.
///
/// # Panics
///
/// If `inputs.len() != out.len()`.
#[inline]
pub fn keccak256_batch(inputs: &[&[u8]], out: &mut [[u8; 32]]) {
    keccak256_batch_with(best_backend(), inputs, out).expect("best backend is supported")
}

/// Like [`keccak256_batch`], with an explicit backend.
///
/// # Panics
///
/// If `inputs.len() != out.len()`.
pub fn keccak256_batch_with(
    backend: Backend,
    inputs: &[&[u8]],
    out: &mut [[u8; 32]],
) -> Result<(), Unsupported> {
    assert_eq!(inputs.len(), out.len(), "inputs and out must have the same length");
    if !backend.is_supported() {
        return Err(Unsupported(backend));
    }
    match backend {
        Backend::Scalar => {
            for (i, o) in inputs.iter().zip(out.iter_mut()) {
                *o = keccak256(i);
            }
        }
        #[cfg(target_arch = "x86_64")]
        // SAFETY: support for each feature was checked above.
        Backend::Avx2 => unsafe { sched::run::<4>(inputs, out, x86::perm_avx2) },
        #[cfg(target_arch = "x86_64")]
        // SAFETY: as above.
        Backend::Avx512Vl4 => unsafe { sched::run::<4>(inputs, out, x86::perm_avx512vl4) },
        #[cfg(target_arch = "x86_64")]
        // SAFETY: as above.
        Backend::Avx512 => unsafe { x86::run_avx512(inputs, out) },
        #[cfg(not(target_arch = "x86_64"))]
        _ => unreachable!("only the scalar backend is supported off x86_64"),
    }
    Ok(())
}

/// Keccak-256 of one message (portable scalar code).
pub fn keccak256(input: &[u8]) -> [u8; 32] {
    scalar::keccak256(input)
}

/// Hashes every `inputs[i]` into `out[i]`, as [`keccak256_batch`].
#[cfg(feature = "alloy")]
pub fn keccak256_batch_b256(inputs: &[&[u8]], out: &mut [alloy_primitives::B256]) {
    // SAFETY: B256 is a repr(transparent) wrapper around [u8; 32].
    let out = unsafe { core::slice::from_raw_parts_mut(out.as_mut_ptr().cast::<[u8; 32]>(), out.len()) };
    keccak256_batch(inputs, out)
}

/// Benchmark hook: runs `iters` permutations of one W-lane state with the given backend and
/// returns a value derived from the state. Not part of the stable API.
#[doc(hidden)]
pub fn __bench_perm(backend: Backend, iters: usize) -> u64 {
    assert!(backend.is_supported());
    match backend {
        Backend::Scalar => {
            let mut s = [1u64; 25];
            for _ in 0..iters {
                scalar::permute(&mut s);
            }
            s[0]
        }
        #[cfg(target_arch = "x86_64")]
        Backend::Avx2 | Backend::Avx512Vl4 => {
            let mut s = sched::State::<4>([[1u64; 4]; 25]);
            let f = if backend == Backend::Avx2 { x86::perm_avx2 as unsafe fn(&mut sched::State<4>) } else { x86::perm_avx512vl4 };
            for _ in 0..iters {
                // SAFETY: checked above.
                unsafe { f(&mut s) };
            }
            s.0[0][0]
        }
        #[cfg(target_arch = "x86_64")]
        Backend::Avx512 => {
            let mut s = sched::State::<8>([[1u64; 8]; 25]);
            for _ in 0..iters {
                // SAFETY: checked above.
                unsafe { x86::perm_avx512(&mut s) };
            }
            s.0[0][0]
        }
        #[cfg(not(target_arch = "x86_64"))]
        _ => unreachable!(),
    }
}


#[cfg(all(target_arch = "x86_64", feature = "exp"))]
mod exp;

/// Benchmark hook for experimental variants.
#[cfg(all(target_arch = "x86_64", feature = "exp"))]
#[doc(hidden)]
pub fn __bench_variants(iters: usize) -> Vec<(&'static str, u64)> {
    let mut v = Vec::new();
    for (name, f) in exp::VARIANTS {
        let mut s = sched::State::<8>([[1u64; 8]; 25]);
        let t = unsafe { core::arch::x86_64::_rdtsc() };
        for _ in 0..iters {
            // SAFETY: bench only, AVX-512 checked by caller.
            unsafe { f(&mut s) };
        }
        let c = unsafe { core::arch::x86_64::_rdtsc() } - t;
        assert_eq!(s.0[0][0], { let mut r = sched::State::<8>([[1u64; 8]; 25]); for _ in 0..iters { unsafe { x86::perm_avx512(&mut r) } }; r.0[0][0] }, "{name} wrong");
        v.push((name, c / iters as u64));
    }
    v
}

/// Benchmark hook for 16-lane experimental variants: cycles per permutation of 16 lanes.
#[cfg(all(target_arch = "x86_64", feature = "exp"))]
#[doc(hidden)]
pub fn __bench_variants16(iters: usize) -> Vec<(&'static str, u64)> {
    let mut v = Vec::new();
    for (name, f) in exp::VARIANTS16 {
        let mut s = sched::State::<16>([[1u64; 16]; 25]);
        let t = unsafe { core::arch::x86_64::_rdtsc() };
        for _ in 0..iters {
            // SAFETY: bench only.
            unsafe { f(&mut s) };
        }
        let c = unsafe { core::arch::x86_64::_rdtsc() } - t;
        let mut r = [1u64; 25];
        for _ in 0..iters { scalar::permute(&mut r); }
        for i in 0..25 { for j in 0..16 { assert_eq!(s.0[i][j], r[i], "{name} wrong"); } }
        v.push((name, c / iters as u64));
    }
    v
}

