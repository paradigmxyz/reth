use crate::{ruint::UintTryFrom, U256};
use alloy_rlp::{Decodable, Encodable, Error};
use reth_codecs::{add_arbitrary_tests, Compact};
use serde::{Deserialize, Serialize};

/// TxValue is the type of the `value` field in the various Ethereum transactions structs.
///
/// While the field is 256 bits, for many chains it's not possible for the field to use
/// this full precision, hence we use a wrapper type to allow for overriding of encoding.
#[add_arbitrary_tests(compact, rlp)]
#[derive(Default, Debug, Copy, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct TxValue(U256);

impl From<TxValue> for U256 {
    #[inline]
    fn from(value: TxValue) -> U256 {
        value.0
    }
}

impl<T> From<T> for TxValue
where
    U256: UintTryFrom<T>,
{
    #[inline]
    #[track_caller]
    fn from(value: T) -> Self {
        Self(U256::uint_try_from(value).unwrap())
    }
}

impl Encodable for TxValue {
    #[inline]
    fn encode(&self, out: &mut dyn bytes::BufMut) {
        self.0.encode(out)
    }

    #[inline]
    fn length(&self) -> usize {
        self.0.length()
    }
}

impl Decodable for TxValue {
    #[inline]
    fn decode(buf: &mut &[u8]) -> Result<Self, Error> {
        #[cfg(feature = "optimism")]
        {
            U256::decode(buf).map(Self)
        }
        #[cfg(not(feature = "optimism"))]
        {
            u128::decode(buf).map(Self::from)
        }
    }
}

/// As ethereum circulation on mainnet is around 120mil eth as of 2022 that is around
/// 120000000000000000000000000 wei we are safe to use u128 for TxValue's encoding
/// as its max number is 340282366920938463463374607431768211455.
/// This optimization should be disabled for chains such as Optimism, where
/// some tx values may require more than 128-bit precision.
impl Compact for TxValue {
    #[inline]
    fn to_compact<B>(self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        #[cfg(feature = "optimism")]
        {
            self.0.to_compact(buf)
        }
        #[cfg(not(feature = "optimism"))]
        {
            self.0.to::<u128>().to_compact(buf)
        }
    }

    #[inline]
    fn from_compact(buf: &[u8], identifier: usize) -> (Self, &[u8]) {
        #[cfg(feature = "optimism")]
        {
            let (i, buf) = U256::from_compact(buf, identifier);
            (TxValue(i), buf)
        }
        #[cfg(not(feature = "optimism"))]
        {
            let (i, buf) = u128::from_compact(buf, identifier);
            (TxValue::from(i), buf)
        }
    }
}

impl std::fmt::Display for TxValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl PartialEq<U256> for TxValue {
    fn eq(&self, other: &U256) -> bool {
        self.0.eq(other)
    }
}

#[cfg(any(test, feature = "arbitrary"))]
impl<'a> arbitrary::Arbitrary<'a> for TxValue {
    #[inline]
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        #[cfg(feature = "optimism")]
        {
            U256::arbitrary(u).map(Self)
        }
        #[cfg(not(feature = "optimism"))]
        {
            u128::arbitrary(u).map(Self::from)
        }
    }
}

#[cfg(any(test, feature = "arbitrary"))]
impl proptest::arbitrary::Arbitrary for TxValue {
    type Parameters = <U256 as proptest::arbitrary::Arbitrary>::Parameters;
    #[inline]
    fn arbitrary_with((): Self::Parameters) -> Self::Strategy {
        use proptest::strategy::Strategy;

        #[cfg(feature = "optimism")]
        {
            proptest::prelude::any::<U256>().prop_map(Self)
        }
        #[cfg(not(feature = "optimism"))]
        {
            proptest::prelude::any::<u128>().prop_map(Self::from)
        }
    }
    #[cfg(feature = "optimism")]
    type Strategy = proptest::arbitrary::Mapped<U256, Self>;

    #[cfg(not(feature = "optimism"))]
    type Strategy = proptest::arbitrary::Mapped<u128, Self>;
}
