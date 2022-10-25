use crate::db::{models::accounts::AccountBeforeTx, Decode, Encode, Error};
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

impl ScaleOnly for Vec<u8> {}
impl sealed::Sealed for Vec<u8> {}

impl_scale!(u8, u32, u16, u64, U256, H256, H160);

impl_scale!(Header, Account, Log, Receipt, TxType, StorageEntry);

impl_scale!(AccountBeforeTx);
