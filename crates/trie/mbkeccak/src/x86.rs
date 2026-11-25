//! x86_64 interleaved Keccak-f[1600] permutations.
#![allow(unused_unsafe)]

use crate::{sched::State, RC};
use core::arch::x86_64::*;

/// 8 states in 512-bit registers: ternary logic for theta and chi, native 64-bit rotates.
#[target_feature(enable = "avx512f")]
pub(crate) unsafe fn perm_avx512(st: &mut State<8>) {
    macro_rules! xor3 {
        ($x:expr, $y:expr, $z:expr) => {
            _mm512_ternarylogic_epi64::<0x96>($x, $y, $z)
        };
    }
    macro_rules! xor {
        ($x:expr, $y:expr) => {
            _mm512_xor_si512($x, $y)
        };
    }
    macro_rules! chi {
        ($x:expr, $y:expr, $z:expr) => {
            _mm512_ternarylogic_epi64::<0xD2>($x, $y, $z)
        };
    }
    macro_rules! rol {
        ($x:expr, $n:literal) => {
            _mm512_rol_epi64::<$n>($x)
        };
    }
    unsafe {
        let p = st.0.as_mut_ptr().cast::<__m512i>();
        let mut a = [_mm512_setzero_si512(); 25];
        for (i, v) in a.iter_mut().enumerate() {
            *v = _mm512_load_si512(p.add(i).cast());
        }
        for rc in RC {
            keccak_round_rmd!(a, xor3, xor, chi, rol);
            a[0] = _mm512_xor_si512(a[0], _mm512_set1_epi64(rc as i64));
        }
        for (i, v) in a.iter().enumerate() {
            _mm512_store_si512(p.add(i).cast(), *v);
        }
    }
}

/// 4 states in 256-bit registers with AVX-512VL ternary logic and rotates.
#[target_feature(enable = "avx512f,avx512vl")]
pub(crate) unsafe fn perm_avx512vl4(st: &mut State<4>) {
    macro_rules! xor3 {
        ($x:expr, $y:expr, $z:expr) => {
            _mm256_ternarylogic_epi64::<0x96>($x, $y, $z)
        };
    }
    macro_rules! xor {
        ($x:expr, $y:expr) => {
            _mm256_xor_si256($x, $y)
        };
    }
    macro_rules! chi {
        ($x:expr, $y:expr, $z:expr) => {
            _mm256_ternarylogic_epi64::<0xD2>($x, $y, $z)
        };
    }
    macro_rules! rol {
        ($x:expr, $n:literal) => {
            _mm256_rol_epi64::<$n>($x)
        };
    }
    unsafe {
        let p = st.0.as_mut_ptr().cast::<__m256i>();
        let mut a = [_mm256_setzero_si256(); 25];
        for (i, v) in a.iter_mut().enumerate() {
            *v = _mm256_load_si256(p.add(i));
        }
        for rc in RC {
            keccak_round_rmd!(a, xor3, xor, chi, rol);
            a[0] = _mm256_xor_si256(a[0], _mm256_set1_epi64x(rc as i64));
        }
        for (i, v) in a.iter().enumerate() {
            _mm256_store_si256(p.add(i), *v);
        }
    }
}

/// 4 states in 256-bit registers with plain AVX2.
#[target_feature(enable = "avx2")]
pub(crate) unsafe fn perm_avx2(st: &mut State<4>) {
    unsafe {
        let rot8 = _mm256_setr_epi8(
            7, 0, 1, 2, 3, 4, 5, 6, 15, 8, 9, 10, 11, 12, 13, 14, 7, 0, 1, 2, 3, 4, 5, 6, 15, 8, 9,
            10, 11, 12, 13, 14,
        );
        let rot56 = _mm256_setr_epi8(
            1, 2, 3, 4, 5, 6, 7, 0, 9, 10, 11, 12, 13, 14, 15, 8, 1, 2, 3, 4, 5, 6, 7, 0, 9, 10,
            11, 12, 13, 14, 15, 8,
        );
        macro_rules! xor {
            ($x:expr, $y:expr) => {
                _mm256_xor_si256($x, $y)
            };
        }
        macro_rules! xor3 {
            ($x:expr, $y:expr, $z:expr) => {
                _mm256_xor_si256(_mm256_xor_si256($x, $y), $z)
            };
        }
        macro_rules! chi {
            ($x:expr, $y:expr, $z:expr) => {
                _mm256_xor_si256($x, _mm256_andnot_si256($y, $z))
            };
        }
        macro_rules! rol {
            ($x:expr, 8) => {
                _mm256_shuffle_epi8($x, rot8)
            };
            ($x:expr, 56) => {
                _mm256_shuffle_epi8($x, rot56)
            };
            ($x:expr, $n:literal) => {{
                let v = $x;
                _mm256_or_si256(_mm256_slli_epi64::<$n>(v), _mm256_srli_epi64::<{ 64 - $n }>(v))
            }};
        }
        let p = st.0.as_mut_ptr().cast::<__m256i>();
        let mut a = [_mm256_setzero_si256(); 25];
        for (i, v) in a.iter_mut().enumerate() {
            *v = _mm256_load_si256(p.add(i));
        }
        for rc in RC {
            keccak_round_rmd!(a, xor3, xor, chi, rol);
            a[0] = _mm256_xor_si256(a[0], _mm256_set1_epi64x(rc as i64));
        }
        for (i, v) in a.iter().enumerate() {
            _mm256_store_si256(p.add(i), *v);
        }
    }
}

/// CPUID/XGETBV feature detection for builds without std.
#[cfg(not(feature = "std"))]
pub(crate) fn detect_raw() -> u8 {
    // SAFETY: CPUID is available on every x86_64 CPU.
    let l1 = unsafe { __cpuid(1) };
    let osxsave = l1.ecx & (1 << 27) != 0;
    let avx = l1.ecx & (1 << 28) != 0;
    if !osxsave || !avx {
        return 0;
    }
    let xcr0: u64 = {
        let (lo, hi): (u32, u32);
        // SAFETY: OSXSAVE is set, so XGETBV is enabled.
        unsafe { core::arch::asm!("xgetbv", in("ecx") 0, out("eax") lo, out("edx") hi, options(nomem, nostack, preserves_flags)) };
        (lo as u64) | ((hi as u64) << 32)
    };
    if xcr0 & 0b110 != 0b110 {
        return 0;
    }
    // SAFETY: as above.
    let l7 = unsafe { __cpuid_count(7, 0) };
    let mut f = 0;
    if l7.ebx & (1 << 5) != 0 {
        f |= crate::F_AVX2;
    }
    let avx512f = l7.ebx & (1 << 16) != 0;
    let avx512vl = l7.ebx & (1 << 31) != 0;
    let avx512bw = l7.ebx & (1 << 30) != 0;
    if avx512f && avx512vl && avx512bw && xcr0 & 0b1110_0000 == 0b1110_0000 {
        f |= crate::F_AVX512;
    }
    f
}

/// Body of the 8-lane AVX-512 permutation on a local `[__m512i; 25]`.
macro_rules! perm8_body {
    ($a:ident) => {{
        macro_rules! xor3 {
            ($x:expr, $y:expr, $z:expr) => {
                _mm512_ternarylogic_epi64::<0x96>($x, $y, $z)
            };
        }
        macro_rules! xor {
            ($x:expr, $y:expr) => {
                _mm512_xor_si512($x, $y)
            };
        }
        macro_rules! chi {
            ($x:expr, $y:expr, $z:expr) => {
                _mm512_ternarylogic_epi64::<0xD2>($x, $y, $z)
            };
        }
        macro_rules! rol {
            ($x:expr, $n:literal) => {
                _mm512_rol_epi64::<$n>($x)
            };
        }
        for rc in RC {
            keccak_round_rmd!($a, xor3, xor, chi, rol);
            $a[0] = _mm512_xor_si512($a[0], _mm512_set1_epi64(rc as i64));
        }
    }};
}

#[inline(always)]
fn low_mask(n: usize) -> u64 {
    if n >= 64 {
        u64::MAX
    } else {
        (1u64 << n) - 1
    }
}

/// Transposes 8 rows of 8 u64 words into 8 columns.
#[inline(always)]
unsafe fn transpose8(r: [__m512i; 8]) -> [__m512i; 8] {
    unsafe {
        let t0 = _mm512_unpacklo_epi64(r[0], r[1]);
        let t1 = _mm512_unpackhi_epi64(r[0], r[1]);
        let t2 = _mm512_unpacklo_epi64(r[2], r[3]);
        let t3 = _mm512_unpackhi_epi64(r[2], r[3]);
        let t4 = _mm512_unpacklo_epi64(r[4], r[5]);
        let t5 = _mm512_unpackhi_epi64(r[4], r[5]);
        let t6 = _mm512_unpacklo_epi64(r[6], r[7]);
        let t7 = _mm512_unpackhi_epi64(r[6], r[7]);
        let u0 = _mm512_shuffle_i64x2::<0x88>(t0, t2);
        let u1 = _mm512_shuffle_i64x2::<0xDD>(t0, t2);
        let u2 = _mm512_shuffle_i64x2::<0x88>(t1, t3);
        let u3 = _mm512_shuffle_i64x2::<0xDD>(t1, t3);
        let u4 = _mm512_shuffle_i64x2::<0x88>(t4, t6);
        let u5 = _mm512_shuffle_i64x2::<0xDD>(t4, t6);
        let u6 = _mm512_shuffle_i64x2::<0x88>(t5, t7);
        let u7 = _mm512_shuffle_i64x2::<0xDD>(t5, t7);
        [
            _mm512_shuffle_i64x2::<0x88>(u0, u4),
            _mm512_shuffle_i64x2::<0x88>(u2, u6),
            _mm512_shuffle_i64x2::<0x88>(u1, u5),
            _mm512_shuffle_i64x2::<0x88>(u3, u7),
            _mm512_shuffle_i64x2::<0xDD>(u0, u4),
            _mm512_shuffle_i64x2::<0xDD>(u2, u6),
            _mm512_shuffle_i64x2::<0xDD>(u1, u5),
            _mm512_shuffle_i64x2::<0xDD>(u3, u7),
        ]
    }
}

const NONE: usize = usize::MAX;

/// Per-slot message cursors of the AVX-512 driver.
struct Lanes {
    msg: [usize; 8],
    ptr: [*const u8; 8],
    rem: [usize; 8],
}

/// Next block of every slot, transposed into lane-word columns.
struct Blocks {
    cols: [__m512i; 17],
    /// Slots starting a new message with this block.
    fresh: u8,
    /// Slots whose message ends with this block.
    fin: u8,
    /// Slots with a block.
    act: u8,
}

/// Refills free slots and loads each slot's next block: masked byte loads (never reading past the
/// message), Keccak padding for final blocks, and two 8x8 transposes.
#[inline(always)]
unsafe fn prep(lanes: &mut Lanes, feeder: &mut crate::sched::Feeder<'_>, inputs: &[&[u8]], b: &mut Blocks) {
    use crate::RATE;
    unsafe {
        let ones = _mm512_set1_epi8(1);
        let mut fresh: u8 = 0;
        let mut fin: u8 = 0;
        let mut act: u8 = 0;
        let mut lo = [_mm512_setzero_si512(); 8];
        let mut hi = [_mm512_setzero_si512(); 8];
        let mut tail = [0u64; 8];
        for j in 0..8 {
            if lanes.msg[j] == NONE {
                match feeder.next() {
                    Some(i) => {
                        lanes.msg[j] = i;
                        lanes.ptr[j] = inputs[i].as_ptr();
                        lanes.rem[j] = inputs[i].len();
                        fresh |= 1 << j;
                    }
                    None => continue,
                }
            }
            act |= 1 << j;
            let n = lanes.rem[j];
            let p = lanes.ptr[j];
            let is_fin = n < RATE;
            let c = if is_fin { n } else { RATE };
            // Bytes of this block that come from the message; the rest are zero.
            let m0 = low_mask(c);
            let m1 = low_mask(c.saturating_sub(64));
            let m2 = low_mask(c.saturating_sub(128)) as u16;
            let mut r0 = _mm512_maskz_loadu_epi8(m0, p.cast());
            let mut r1 = _mm512_maskz_loadu_epi8(m1, p.wrapping_add(64).cast());
            let t = _mm_maskz_loadu_epi8(m2, p.wrapping_add(128).cast());
            let mut t = _mm_cvtsi128_si64(t) as u64;
            // Keccak padding: 0x01 after the message, 0x80 in the last byte of the block.
            let k0 = if is_fin && n < 64 { 1u64 << n } else { 0 };
            let k1 = if is_fin && (64..128).contains(&n) { 1u64 << (n - 64) } else { 0 };
            let k2 = if is_fin && n >= 128 { 1u64 << (8 * (n - 128)) } else { 0 };
            r0 = _mm512_mask_mov_epi8(r0, k0, ones);
            r1 = _mm512_mask_mov_epi8(r1, k1, ones);
            t |= k2 | ((is_fin as u64) << 63);
            lo[j] = r0;
            hi[j] = r1;
            tail[j] = t;
            fin |= (is_fin as u8) << j;
            lanes.ptr[j] = p.wrapping_add(c);
            lanes.rem[j] = n - c;
        }
        let c0 = transpose8(lo);
        let c1 = transpose8(hi);
        b.cols[..8].copy_from_slice(&c0);
        b.cols[8..16].copy_from_slice(&c1);
        b.cols[16] = _mm512_loadu_si512(tail.as_ptr().cast());
        b.fresh = fresh;
        b.fin = fin;
        b.act = act;
    }
}

/// Whole batch on AVX-512, software-pipelined: the next blocks are loaded and transposed before
/// the current permutation runs (they do not depend on it), so the out-of-order core overlaps
/// that scalar and shuffle work with the permutation's ternlog/rotate work.
#[target_feature(enable = "avx512f,avx512bw,avx512vl")]
pub(crate) unsafe fn run_avx512(inputs: &[&[u8]], out: &mut [[u8; 32]]) {
    unsafe {
        let mut feeder = crate::sched::Feeder::new(inputs);
        let mut lanes = Lanes { msg: [NONE; 8], ptr: [core::ptr::null(); 8], rem: [0; 8] };
        let mut b = Blocks { cols: [_mm512_setzero_si512(); 17], fresh: 0, fin: 0, act: 0 };
        let mut s = [_mm512_setzero_si512(); 25];
        prep(&mut lanes, &mut feeder, inputs, &mut b);
        while b.act != 0 {
            let keep: __mmask8 = !b.fresh;
            for w in 0..17 {
                s[w] = _mm512_mask_xor_epi64(b.cols[w], keep, s[w], b.cols[w]);
            }
            for w in 17..25 {
                s[w] = _mm512_maskz_mov_epi64(keep, s[w]);
            }
            let fin = b.fin;
            let msg = lanes.msg;
            let mut f = fin;
            while f != 0 {
                lanes.msg[f.trailing_zeros() as usize] = NONE;
                f &= f - 1;
            }
            prep(&mut lanes, &mut feeder, inputs, &mut b);
            perm8_body!(s);
            if fin != 0 {
                let mut buf = [[0u64; 8]; 4];
                for w in 0..4 {
                    _mm512_storeu_si512(buf[w].as_mut_ptr().cast(), s[w]);
                }
                let mut f = fin;
                while f != 0 {
                    let j = f.trailing_zeros() as usize;
                    f &= f - 1;
                    let o = &mut out[msg[j]];
                    for w in 0..4 {
                        o[w * 8..w * 8 + 8].copy_from_slice(&buf[w][j].to_le_bytes());
                    }
                }
            }
        }
    }
}
