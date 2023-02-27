//! Bloom type.
//!
//! Adapted from <https://github.com/paritytech/parity-common/blob/2fb72eea96b6de4a085144ce239feb49da0cd39e/ethbloom/src/lib.rs>
#![allow(missing_docs)]
use crate::{impl_fixed_hash_type, keccak256, Log};
use bytes::Buf;
use core::{mem, ops};
use crunchy::unroll;
use derive_more::{AsRef, Deref};
use fixed_hash::*;
use impl_serde::impl_fixed_hash_serde;
use reth_codecs::{impl_hash_compact, Compact};
use reth_rlp::{RlpDecodableWrapper, RlpEncodableWrapper, RlpMaxEncodedLen};

/// Length of bloom filter used for Ethereum.
pub const BLOOM_BITS: u32 = 3;
pub const BLOOM_SIZE: usize = 256;

impl_fixed_hash_type!((Bloom, BLOOM_SIZE));

/// Returns log2.
fn log2(x: usize) -> u32 {
    if x <= 1 {
        return 0
    }

    let n = x.leading_zeros();
    mem::size_of::<usize>() as u32 * 8 - n
}

#[derive(Debug)]
pub enum Input<'a> {
    Raw(&'a [u8]),
    Hash(&'a [u8; 32]),
}

enum Hash<'a> {
    Ref(&'a [u8; 32]),
    Owned([u8; 32]),
}

impl<'a> From<Input<'a>> for Hash<'a> {
    fn from(input: Input<'a>) -> Self {
        match input {
            Input::Raw(raw) => Hash::Owned(keccak256(raw).0),
            Input::Hash(hash) => Hash::Ref(hash),
        }
    }
}

impl<'a> ops::Index<usize> for Hash<'a> {
    type Output = u8;

    fn index(&self, index: usize) -> &u8 {
        match *self {
            Hash::Ref(r) => &r[index],
            Hash::Owned(ref hash) => &hash[index],
        }
    }
}

impl<'a> Hash<'a> {
    fn len(&self) -> usize {
        match *self {
            Hash::Ref(r) => r.len(),
            Hash::Owned(ref hash) => hash.len(),
        }
    }
}

impl<'a> PartialEq<BloomRef<'a>> for Bloom {
    fn eq(&self, other: &BloomRef<'a>) -> bool {
        let s_ref: &[u8] = &self.0;
        let o_ref: &[u8] = other.0;
        s_ref.eq(o_ref)
    }
}

impl<'a> From<Input<'a>> for Bloom {
    fn from(input: Input<'a>) -> Bloom {
        let mut bloom = Bloom::default();
        bloom.accrue(input);
        bloom
    }
}

impl Bloom {
    pub fn contains_bloom<'a, B>(&self, bloom: B) -> bool
    where
        BloomRef<'a>: From<B>,
    {
        let bloom_ref: BloomRef<'_> = bloom.into();
        self.contains_bloom_ref(bloom_ref)
    }

    fn contains_bloom_ref(&self, bloom: BloomRef<'_>) -> bool {
        let self_ref: BloomRef<'_> = self.into();
        self_ref.contains_bloom(bloom)
    }

    pub fn accrue(&mut self, input: Input<'_>) {
        let p = BLOOM_BITS;

        let m = self.0.len();
        let bloom_bits = m * 8;
        let mask = bloom_bits - 1;
        let bloom_bytes = (log2(bloom_bits) + 7) / 8;

        let hash: Hash<'_> = input.into();

        // must be a power of 2
        assert_eq!(m & (m - 1), 0);
        // out of range
        assert!(p * bloom_bytes <= hash.len() as u32);

        let mut ptr = 0;

        assert_eq!(BLOOM_BITS, 3);
        unroll! {
            for i in 0..3 {
                let _ = i;
                let mut index = 0_usize;
                for _ in 0..bloom_bytes {
                    index = (index << 8) | hash[ptr] as usize;
                    ptr += 1;
                }
                index &= mask;
                self.0[m - 1 - index / 8] |= 1 << (index % 8);
            }
        }
    }

    pub fn accrue_bloom<'a, B>(&mut self, bloom: B)
    where
        BloomRef<'a>: From<B>,
    {
        let bloom_ref: BloomRef<'_> = bloom.into();
        assert_eq!(self.0.len(), BLOOM_SIZE);
        assert_eq!(bloom_ref.0.len(), BLOOM_SIZE);
        for i in 0..BLOOM_SIZE {
            self.0[i] |= bloom_ref.0[i];
        }
    }

    pub fn data(&self) -> &[u8; BLOOM_SIZE] {
        &self.0
    }
}

#[derive(Clone, Copy, Debug)]
pub struct BloomRef<'a>(&'a [u8; BLOOM_SIZE]);

impl<'a> BloomRef<'a> {
    /// Returns `true` if bloom only consists of `0`
    pub fn is_empty(&self) -> bool {
        self.0.iter().all(|x| *x == 0)
    }

    pub fn contains_bloom<'b, B>(&self, bloom: B) -> bool
    where
        BloomRef<'b>: From<B>,
    {
        let bloom_ref: BloomRef<'_> = bloom.into();
        assert_eq!(self.0.len(), BLOOM_SIZE);
        assert_eq!(bloom_ref.0.len(), BLOOM_SIZE);
        for i in 0..BLOOM_SIZE {
            let a = self.0[i];
            let b = bloom_ref.0[i];
            if (a & b) != b {
                return false
            }
        }
        true
    }

    pub fn data(&self) -> &'a [u8; BLOOM_SIZE] {
        self.0
    }
}

impl<'a> From<&'a [u8; BLOOM_SIZE]> for BloomRef<'a> {
    fn from(data: &'a [u8; BLOOM_SIZE]) -> Self {
        BloomRef(data)
    }
}

impl<'a> From<&'a Bloom> for BloomRef<'a> {
    fn from(bloom: &'a Bloom) -> Self {
        BloomRef(&bloom.0)
    }
}

// See Section 4.3.1 "Transaction Receipt" of the Yellow Paper
fn m3_2048(bloom: &mut Bloom, x: &[u8]) {
    let hash = keccak256(x);
    let h: &[u8; 32] = hash.as_ref();
    for i in [0, 2, 4] {
        let bit = (h[i + 1] as usize + ((h[i] as usize) << 8)) & 0x7FF;
        bloom.0[BLOOM_SIZE - 1 - bit / 8] |= 1 << (bit % 8);
    }
}

/// Calculate receipt logs bloom.
pub fn logs_bloom<'a, It>(logs: It) -> Bloom
where
    It: IntoIterator<Item = &'a Log>,
{
    let mut bloom = Bloom::zero();
    for log in logs {
        m3_2048(&mut bloom, log.address.as_bytes());
        for topic in &log.topics {
            m3_2048(&mut bloom, topic.as_bytes());
        }
    }
    bloom
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hex_literal::hex;

    #[test]
    fn hardcoded_bloom() {
        let logs = vec![
            Log {
                address: hex!("22341ae42d6dd7384bc8584e50419ea3ac75b83f").into(),
                topics: vec![hex!(
                    "04491edcd115127caedbd478e2e7895ed80c7847e903431f94f9cfa579cad47f"
                )
                .into()],
                data: vec![].into(),
            },
            Log {
                address: hex!("e7fb22dfef11920312e4989a3a2b81e2ebf05986").into(),
                topics: vec![
                    hex!("7f1fef85c4b037150d3675218e0cdb7cf38fea354759471e309f3354918a442f").into(),
                    hex!("d85629c7eaae9ea4a10234fed31bc0aeda29b2683ebe0c1882499d272621f6b6").into(),
                ],
                data: hex::decode("2d690516512020171c1ec870f6ff45398cc8609250326be89915fb538e7b")
                    .unwrap()
                    .into(),
            },
        ];
        assert_eq!(
            logs_bloom(&logs),
            Bloom::from(hex!(
                "000000000000000000810000000000000000000000000000000000020000000000000000000000000000008000"
                "000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000"
                "000000000000000000000000000000000000000200000000000000000000000000000000000000000000000000"
                "000000000000000000000000000000000000000000000000000000280000000000400000800000004000000000"
                "000000000000000000000000000000000000000000000000000000000000100000100000000000000000000000"
                "00000000001400000000000000008000000000000000000000000000000000"
            ))
        );
    }
}
