use crate::db::{Decode, Encode, Error};
use parity_scale_codec::decode_from_bytes;
use reth_primitives::*;

mod sealed {
    pub trait Sealed {}
}
/// Marker trait type to restrict the TableEncode and TableDecode with scale to chosen types.
pub trait ScaleOnly: sealed::Sealed {}

impl<T> Encode for T
where
    T: ScaleOnly + parity_scale_codec::Encode + Sync + Send + std::fmt::Debug,
{
    type Encoded = Vec<u8>;

    fn encode(self) -> Self::Encoded {
        parity_scale_codec::Encode::encode(&self)
    }
}

impl<T> Decode for T
where
    T: ScaleOnly + parity_scale_codec::Decode + Sync + Send + std::fmt::Debug,
{
    fn decode<B: Into<bytes::Bytes>>(value: B) -> Result<T, Error> {
        decode_from_bytes(value.into()).map_err(|e| Error::Decode(e.into()))
    }
}

macro_rules! impl_scale {
    ($($name:tt),+) => {
        $(
            impl ScaleOnly for $name {}
            impl sealed::Sealed for $name {}
        )+
    };
}

impl_scale!(u16, H256, U256, H160, u8, u64, Header, Account, Log, Receipt, TxType);

impl ScaleOnly for Vec<u8> {}
impl sealed::Sealed for Vec<u8> {}
