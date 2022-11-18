use crate::db::{Compress, Decompress, Error};
use parity_scale_codec::decode_from_bytes;
use reth_primitives::*;

mod sealed {
    pub trait Sealed {}
}

/// Marker trait type to restrict the [`Compress`] and [`Decompress`] with scale to chosen types.
pub trait ScaleValue: sealed::Sealed {}

impl<T> Compress for T
where
    T: ScaleValue + parity_scale_codec::Encode + Sync + Send + std::fmt::Debug,
{
    type Compressed = Vec<u8>;

    fn compress(self) -> Self::Compressed {
        parity_scale_codec::Encode::encode(&self)
    }
}

impl<T> Decompress for T
where
    T: ScaleValue + parity_scale_codec::Decode + Sync + Send + std::fmt::Debug,
{
    fn decompress<B: Into<bytes::Bytes>>(value: B) -> Result<T, Error> {
        decode_from_bytes(value.into()).map_err(|_| Error::DecodeError)
    }
}

/// Implements compression for SCALE type.
macro_rules! impl_compression_for_scale {
    ($($name:tt),+) => {
        $(
            impl ScaleValue for $name {}
            impl sealed::Sealed for $name {}
        )+
    };
}

impl ScaleValue for Vec<u8> {}
impl sealed::Sealed for Vec<u8> {}

impl_compression_for_scale!(U256, H256, H160);
impl_compression_for_scale!(TransactionSigned);
impl_compression_for_scale!(u8, u32, u16, u64);
