//! Bloom related utilities.
use crate::{keccak256, Log};
use bytes::Buf;
use derive_more::{AsRef, Deref};
use fixed_hash::construct_fixed_hash;
use impl_serde::impl_fixed_hash_serde;
use reth_codecs::{impl_hash_compact, Compact};
use reth_rlp::{RlpDecodableWrapper, RlpEncodableWrapper};

#[cfg(any(test, feature = "arbitrary"))]
use proptest::{
    arbitrary::{any_with, Arbitrary as PropTestArbitrary, ParamsFor},
    strategy::{BoxedStrategy, Strategy},
};

#[cfg(any(test, feature = "arbitrary"))]
use arbitrary::Arbitrary;

/// Length of bloom filter used for Ethereum.
pub const BLOOM_BYTE_LENGTH: usize = 256;

construct_fixed_hash! {
    /// 2048 bits type.
    #[cfg_attr(any(test, feature = "arbitrary"), derive(Arbitrary))]
    #[derive(AsRef, Deref, RlpEncodableWrapper, RlpDecodableWrapper)]
    pub struct Bloom(BLOOM_BYTE_LENGTH);
}

impl_hash_compact!(Bloom);
impl_fixed_hash_serde!(Bloom, BLOOM_BYTE_LENGTH);

#[cfg(any(test, feature = "arbitrary"))]
impl PropTestArbitrary for Bloom {
    type Parameters = ParamsFor<u8>;
    type Strategy = BoxedStrategy<Bloom>;

    fn arbitrary_with(args: Self::Parameters) -> Self::Strategy {
        proptest::collection::vec(any_with::<u8>(args), BLOOM_BYTE_LENGTH)
            .prop_map(move |vec| Bloom::from_slice(&vec))
            .boxed()
    }
}

// See Section 4.3.1 "Transaction Receipt" of the Yellow Paper
fn m3_2048(bloom: &mut Bloom, x: &[u8]) {
    let hash = keccak256(x);
    let h: &[u8; 32] = hash.as_ref();
    for i in [0, 2, 4] {
        let bit = (h[i + 1] as usize + ((h[i] as usize) << 8)) & 0x7FF;
        bloom.0[BLOOM_BYTE_LENGTH - 1 - bit / 8] |= 1 << (bit % 8);
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
    #[test]
    fn arbitrary() {
        proptest::proptest!(|(bloom: Bloom)| {
            let mut buf = vec![];
            bloom.to_compact(&mut buf);

            // Add noise
            buf.push(1);

            let (decoded, remaining_buf) = Bloom::from_compact(&buf, buf.len());

            assert!(bloom == decoded);
            assert!(remaining_buf.len() == 1);
        });
    }
}
