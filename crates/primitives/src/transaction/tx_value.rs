#[allow(unused_imports)]
// suppress warning for UIntTryTo, which is required only when value-256 feature is disabled
use crate::{
    ruint::{ToUintError, UintTryFrom, UintTryTo},
    U256,
};
use alloy_rlp::{Decodable, Encodable, Error};
use reth_codecs::{add_arbitrary_tests, Compact};
use serde::{Deserialize, Serialize};

#[cfg(any(test, feature = "arbitrary"))]
use proptest::{
    arbitrary::ParamsFor,
    strategy::{BoxedStrategy, Strategy},
};

/// TxValue is the type of the `value` field in the various Ethereum transactions structs.
/// While the field is 256 bits, for many chains it's not possible for the field to use
/// this full precision, hence we use a wrapper type to allow for overriding of encoding.
#[add_arbitrary_tests(compact, rlp)]
#[derive(Default, Debug, Copy, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct TxValue(U256);

impl From<TxValue> for U256 {
    /// unwrap Value to U256
    fn from(value: TxValue) -> U256 {
        value.0
    }
}

impl<T> From<T> for TxValue
where
    Self: UintTryFrom<T>,
{
    /// construct a Value from misc. other uint types
    fn from(value: T) -> Self {
        match Self::uint_try_from(value) {
            Ok(n) => n,
            Err(e) => panic!("Uint conversion error: {e}"),
        }
    }
}

impl UintTryFrom<U256> for TxValue {
    #[inline]
    fn uint_try_from(value: U256) -> Result<Self, ToUintError<Self>> {
        Ok(Self(value))
    }
}

impl UintTryFrom<u128> for TxValue {
    #[inline]
    fn uint_try_from(value: u128) -> Result<Self, ToUintError<Self>> {
        Ok(Self(U256::from(value)))
    }
}

impl UintTryFrom<u64> for TxValue {
    #[inline]
    fn uint_try_from(value: u64) -> Result<Self, ToUintError<Self>> {
        Ok(Self(U256::from(value)))
    }
}

impl Encodable for TxValue {
    fn encode(&self, out: &mut dyn bytes::BufMut) {
        self.0.encode(out)
    }
    fn length(&self) -> usize {
        self.0.length()
    }
}

impl Decodable for TxValue {
    fn decode(buf: &mut &[u8]) -> Result<Self, Error> {
        Ok(TxValue(U256::decode(buf)?))
    }
}

/// As ethereum circulation on mainnet is around 120mil eth as of 2022 that is around
/// 120000000000000000000000000 wei we are safe to use u128 for TxValue's encoding
/// as its max number is 340282366920938463463374607431768211455.
/// This optimization should be disabled for chains such as Optimism, where
/// some tx values may require more than 128-bit precision.
impl Compact for TxValue {
    #[allow(unreachable_code)]
    fn to_compact<B>(self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        #[cfg(feature = "value-256")]
        {
            self.0.to_compact(buf)
        }
        #[cfg(not(feature = "value-256"))]
        {
            // SAFETY: For ethereum mainnet this is safe as the max value is
            // 120000000000000000000000000 wei
            let i: u128 = self.0.uint_try_to().expect("value could not be converted to u128");
            i.to_compact(buf)
        }
    }

    #[allow(unreachable_code)]
    fn from_compact(buf: &[u8], identifier: usize) -> (Self, &[u8]) {
        #[cfg(feature = "value-256")]
        {
            let (i, buf) = U256::from_compact(buf, identifier);
            (TxValue(i), buf)
        }
        #[cfg(not(feature = "value-256"))]
        {
            let (i, buf) = u128::from_compact(buf, identifier);
            (TxValue::from(i), buf)
        }
    }
}

#[cfg(any(test, feature = "arbitrary"))]
impl<'a> arbitrary::Arbitrary<'a> for TxValue {
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        #[cfg(feature = "value-256")]
        {
            Ok(Self(U256::arbitrary(u)?))
        }

        #[cfg(not(feature = "value-256"))]
        {
            Ok(Self::try_from(u128::arbitrary(u)?).expect("to fit"))
        }
    }
}

#[cfg(any(test, feature = "arbitrary"))]
impl proptest::arbitrary::Arbitrary for TxValue {
    type Parameters = ParamsFor<()>;
    fn arbitrary_with(_: Self::Parameters) -> Self::Strategy {
        #[cfg(feature = "value-256")]
        {
            proptest::prelude::any::<U256>().prop_map(Self).boxed()
        }

        #[cfg(not(feature = "value-256"))]
        {
            proptest::prelude::any::<u128>()
                .prop_map(|num| Self::try_from(num).expect("to fit"))
                .boxed()
        }
    }

    type Strategy = BoxedStrategy<TxValue>;
}
