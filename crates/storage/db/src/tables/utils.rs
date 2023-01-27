//! Small database table utilities and helper functions
use crate::{
    table::{Decode, Decompress, Table},
    Error,
};
use bytes::Bytes;
use std::borrow::Cow;

#[macro_export]
/// Implements the `Arbitrary` trait for types with fixed array
/// types.
macro_rules! impl_fixed_arbitrary {
    ($name:tt, $size:tt) => {
        #[cfg(any(test, feature = "arbitrary"))]
        use arbitrary::{Arbitrary, Unstructured};

        #[cfg(any(test, feature = "arbitrary"))]
        impl<'a> Arbitrary<'a> for $name {
            fn arbitrary(u: &mut Unstructured<'a>) -> Result<Self, arbitrary::Error> {
                let mut buffer = vec![0; $size];
                u.fill_buffer(buffer.as_mut_slice())?;

                Decode::decode(buffer).map_err(|_| arbitrary::Error::IncorrectFormat)
            }
        }
    };
}

/// Helper function to decode a `(key, value)` pair.
pub(crate) fn decoder<'a, T>(
    kv: (Cow<'a, [u8]>, Cow<'a, [u8]>),
) -> Result<(T::Key, T::Value), Error>
where
    T: Table,
    T::Key: Decode,
    T::Value: Decompress,
{
    Ok((
        Decode::decode(Bytes::from(kv.0.into_owned()))?,
        Decompress::decompress(Bytes::from(kv.1.into_owned()))?,
    ))
}

/// Helper function to decode only a value from a `(key, value)` pair.
pub(crate) fn decode_value<'a, T>(kv: (Cow<'a, [u8]>, Cow<'a, [u8]>)) -> Result<T::Value, Error>
where
    T: Table,
{
    Decompress::decompress(Bytes::from(kv.1.into_owned()))
}

/// Helper function to decode a value. It can be a key or subkey.
pub(crate) fn decode_one<T>(value: Cow<'_, [u8]>) -> Result<T::Value, Error>
where
    T: Table,
{
    Decompress::decompress(Bytes::from(value.into_owned()))
}
