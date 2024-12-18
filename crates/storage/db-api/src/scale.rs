use crate::{
    table::{Compress, Decompress},
    DatabaseError,
};
use alloy_primitives::U256;

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

    fn compress_to_buf<B: bytes::BufMut + AsMut<[u8]>>(self, buf: &mut B) {
        parity_scale_codec::Encode::encode_to(&self, OutputCompat::wrap_mut(buf));
    }
}

impl<T> Decompress for T
where
    T: ScaleValue + parity_scale_codec::Decode + Sync + Send + std::fmt::Debug,
{
    fn decompress(mut value: &[u8]) -> Result<T, DatabaseError> {
        parity_scale_codec::Decode::decode(&mut value).map_err(|_| DatabaseError::Decode)
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

impl_compression_for_scale!(U256);
impl_compression_for_scale!(u8, u32, u16, u64);

#[repr(transparent)]
struct OutputCompat<B>(B);

impl<B> OutputCompat<B> {
    fn wrap_mut(buf: &mut B) -> &mut Self {
        unsafe { std::mem::transmute(buf) }
    }
}

impl<B: bytes::BufMut> parity_scale_codec::Output for OutputCompat<B> {
    fn write(&mut self, bytes: &[u8]) {
        self.0.put_slice(bytes);
    }

    fn push_byte(&mut self, byte: u8) {
        self.0.put_u8(byte);
    }
}
