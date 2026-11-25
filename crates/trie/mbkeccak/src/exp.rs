//! Experimental permutation variants for hill-climbing (not used by the public API).
#![allow(unused_unsafe, unused_macros)]
use crate::{sched::State, RC};
use core::arch::x86_64::*;

macro_rules! zx3 { ($x:expr, $y:expr, $z:expr) => { _mm512_ternarylogic_epi64::<0x96>($x, $y, $z) }; }
macro_rules! zx { ($x:expr, $y:expr) => { _mm512_xor_si512($x, $y) }; }
macro_rules! zchi_t { ($x:expr, $y:expr, $z:expr) => { _mm512_ternarylogic_epi64::<0xD2>($x, $y, $z) }; }
macro_rules! zchi_a { ($x:expr, $y:expr, $z:expr) => { _mm512_xor_si512($x, _mm512_andnot_si512($y, $z)) }; }
macro_rules! zrol { ($x:expr, $n:literal) => { _mm512_rol_epi64::<$n>($x) }; }

macro_rules! zperm {
    ($name:ident, |$a:ident| $body:block) => {
        #[target_feature(enable = "avx512f")]
        pub(crate) unsafe fn $name(st: &mut State<8>) {
            unsafe {
                let p = st.0.as_mut_ptr().cast::<__m512i>();
                let mut $a = [_mm512_setzero_si512(); 25];
                for (i, v) in $a.iter_mut().enumerate() { *v = _mm512_load_si512(p.add(i).cast()); }
                for rc in RC {
                    $body
                    $a[0] = _mm512_xor_si512($a[0], _mm512_set1_epi64(rc as i64));
                }
                for (i, v) in $a.iter().enumerate() { _mm512_store_si512(p.add(i).cast(), *v); }
            }
        }
    };
}

zperm!(z_ttt, |a| { keccak_round!(a, zx3, zchi_t, zrol); });
zperm!(z_tdt, |a| { keccak_round_cd!(a, zx3, zx, zchi_t, zrol); });
zperm!(z_tda, |a| { keccak_round_cd!(a, zx3, zx, zchi_a, zrol); });
zperm!(z_xdt, |a| { keccak_round_d!(a, zx, zchi_t, zrol); });
zperm!(z_xda, |a| { keccak_round_d!(a, zx, zchi_a, zrol); });

pub(crate) const VARIANTS: [(&str, unsafe fn(&mut State<8>)); 14] =
    [("ttt", z_ttt), ("tdt", z_tdt), ("tda", z_tda), ("xdt", z_xdt), ("xda", z_xda), ("rm", z_rm), ("rmd", z_rmd), ("rmd_a", z_rmd_a), ("rmdc", z_rmdc), ("rmd_u2", z_rmd_u2), ("rmd_u3", z_rmd_u3), ("rmd_u4", z_rmd_u4), ("rmd_u24", z_rmd_u24), ("rmda_u2", z_rmda_u2)];

macro_rules! zperm16 {
    ($name:ident, |$a:ident, $b:ident| $body:block) => {
        #[target_feature(enable = "avx512f")]
        pub(crate) unsafe fn $name(st: &mut State<16>) {
            unsafe {
                let p = st.0.as_mut_ptr().cast::<__m512i>();
                let mut $a = [_mm512_setzero_si512(); 25];
                let mut $b = [_mm512_setzero_si512(); 25];
                for i in 0..25 { $a[i] = _mm512_load_si512(p.add(2 * i).cast()); $b[i] = _mm512_load_si512(p.add(2 * i + 1).cast()); }
                for rc in RC {
                    $body
                    let k = _mm512_set1_epi64(rc as i64);
                    $a[0] = _mm512_xor_si512($a[0], k);
                    $b[0] = _mm512_xor_si512($b[0], k);
                }
                for i in 0..25 { _mm512_store_si512(p.add(2 * i).cast(), $a[i]); _mm512_store_si512(p.add(2 * i + 1).cast(), $b[i]); }
            }
        }
    };
}
zperm16!(z16_ttt, |a, b| { keccak_round!(a, zx3, zchi_t, zrol); keccak_round!(b, zx3, zchi_t, zrol); });
zperm16!(z16_tdt, |a, b| { keccak_round_cd!(a, zx3, zx, zchi_t, zrol); keccak_round_cd!(b, zx3, zx, zchi_t, zrol); });
zperm16!(z16_xda, |a, b| { keccak_round_d!(a, zx, zchi_a, zrol); keccak_round_d!(b, zx, zchi_a, zrol); });

pub(crate) const VARIANTS16: [(&str, unsafe fn(&mut State<16>)); 3] = [("16ttt", z16_ttt), ("16tdt", z16_tdt), ("16xda", z16_xda)];


zperm!(z_rm, |a| { keccak_round_rm!(a, zx3, zchi_t, zrol); });
zperm!(z_rmd, |a| { keccak_round_rmd!(a, zx3, zx, zchi_t, zrol); });
zperm!(z_rmd_a, |a| { keccak_round_rmd!(a, zx3, zx, zchi_a, zrol); });


#[target_feature(enable = "avx512f")]
pub(crate) unsafe fn z_rmdc(st: &mut State<8>) {
    unsafe {
        let p = st.0.as_mut_ptr().cast::<__m512i>();
        let mut a = [_mm512_setzero_si512(); 25];
        for (i, v) in a.iter_mut().enumerate() { *v = _mm512_load_si512(p.add(i).cast()); }
        let mut c = [_mm512_setzero_si512(); 5];
        for x in 0..5 { c[x] = zx3!(zx3!(a[x], a[x + 5], a[x + 10]), a[x + 15], a[x + 20]); }
        for rc in RC {
            macro_rules! iota { ($e:expr) => { _mm512_xor_si512($e, _mm512_set1_epi64(rc as i64)) }; }
            keccak_round_rmdc!(a, c, zx3, zx, zchi_t, zrol, iota);
        }
        for (i, v) in a.iter().enumerate() { _mm512_store_si512(p.add(i).cast(), *v); }
    }
}


macro_rules! zpermu {
    ($name:ident, $u:literal, |$a:ident| $body:block) => {
        #[target_feature(enable = "avx512f")]
        pub(crate) unsafe fn $name(st: &mut State<8>) {
            unsafe {
                let p = st.0.as_mut_ptr().cast::<__m512i>();
                let mut $a = [_mm512_setzero_si512(); 25];
                for (i, v) in $a.iter_mut().enumerate() { *v = _mm512_load_si512(p.add(i).cast()); }
                for rcs in RC.chunks_exact($u) {
                    for k in 0..$u {
                        $body
                        $a[0] = _mm512_xor_si512($a[0], _mm512_set1_epi64(rcs[k] as i64));
                    }
                }
                for (i, v) in $a.iter().enumerate() { _mm512_store_si512(p.add(i).cast(), *v); }
            }
        }
    };
}
zpermu!(z_rmd_u2, 2, |a| { keccak_round_rmd!(a, zx3, zx, zchi_t, zrol); });
zpermu!(z_rmd_u3, 3, |a| { keccak_round_rmd!(a, zx3, zx, zchi_t, zrol); });
zpermu!(z_rmd_u4, 4, |a| { keccak_round_rmd!(a, zx3, zx, zchi_t, zrol); });
zpermu!(z_rmd_u24, 24, |a| { keccak_round_rmd!(a, zx3, zx, zchi_t, zrol); });
zpermu!(z_rmda_u2, 2, |a| { keccak_round_rmd!(a, zx3, zx, zchi_a, zrol); });

