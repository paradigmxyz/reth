//! Portable Keccak-f[1600] and a one-message sponge.

use crate::{RATE, RC};

#[inline(always)]
pub(crate) fn permute(a: &mut [u64; 25]) {
    macro_rules! xor3 {
        ($x:expr, $y:expr, $z:expr) => {
            $x ^ $y ^ $z
        };
    }
    macro_rules! chi {
        ($x:expr, $y:expr, $z:expr) => {
            $x ^ (!$y & $z)
        };
    }
    macro_rules! rol {
        ($x:expr, $n:literal) => {
            $x.rotate_left($n)
        };
    }
    let mut s = *a;
    for rc in RC {
        keccak_round!(s, xor3, chi, rol);
        s[0] ^= rc;
    }
    *a = s;
}

#[inline(always)]
pub(crate) fn absorb_full(s: &mut [u64; 25], block: &[u8]) {
    for (w, c) in s.iter_mut().zip(block[..RATE].chunks_exact(8)) {
        *w ^= u64::from_le_bytes(c.try_into().unwrap());
    }
}

pub(crate) fn keccak256(input: &[u8]) -> [u8; 32] {
    let mut s = [0u64; 25];
    let mut chunks = input.chunks_exact(RATE);
    for b in &mut chunks {
        absorb_full(&mut s, b);
        permute(&mut s);
    }
    let rem = chunks.remainder();
    let mut last = [0u8; RATE];
    last[..rem.len()].copy_from_slice(rem);
    last[rem.len()] ^= 0x01;
    last[RATE - 1] ^= 0x80;
    absorb_full(&mut s, &last);
    permute(&mut s);
    let mut out = [0u8; 32];
    for (o, w) in out.chunks_exact_mut(8).zip(&s[..4]) {
        o.copy_from_slice(&w.to_le_bytes());
    }
    out
}
