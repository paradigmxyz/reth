//! Small database table utilities and helper functions.

use crate::{
    table::{Decode, Decompress, Table, TableRow},
    DatabaseError,
};
use std::borrow::Cow;

#[macro_export]
/// Implements the `Arbitrary` trait for types with fixed array types.
macro_rules! impl_fixed_arbitrary {
    ($name:ident, $size:tt) => {
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

        #[cfg(any(test, feature = "arbitrary"))]
        impl proptest::prelude::Arbitrary for $name {
            type Parameters = ();
            type Strategy = proptest::strategy::Map<
                proptest::collection::VecStrategy<<u8 as proptest::arbitrary::Arbitrary>::Strategy>,
                fn(Vec<u8>) -> Self,
            >;

            fn arbitrary_with(args: Self::Parameters) -> Self::Strategy {
                use proptest::strategy::Strategy;
                proptest::collection::vec(proptest::arbitrary::any_with::<u8>(args), $size)
                    .prop_map(move |vec| Decode::decode(vec).unwrap())
            }
        }
    };
}

/// Helper function to decode a `(key, value)` pair.
pub(crate) fn decoder<'a, T>(
    kv: (Cow<'a, [u8]>, Cow<'a, [u8]>),
) -> Result<TableRow<T>, DatabaseError>
where
    T: Table,
    T::Key: Decode,
    T::Value: Decompress,
{
    let key = match kv.0 {
        Cow::Borrowed(k) => Decode::decode(k)?,
        Cow::Owned(k) => Decode::decode(k)?,
    };
    let value = match kv.1 {
        Cow::Borrowed(v) => Decompress::decompress(v)?,
        Cow::Owned(v) => Decompress::decompress_owned(v)?,
    };
    Ok((key, value))
}

/// Helper function to decode only a value from a `(key, value)` pair.
pub(crate) fn decode_value<'a, T>(
    kv: (Cow<'a, [u8]>, Cow<'a, [u8]>),
) -> Result<T::Value, DatabaseError>
where
    T: Table,
{
    Ok(match kv.1 {
        Cow::Borrowed(v) => Decompress::decompress(v)?,
        Cow::Owned(v) => Decompress::decompress_owned(v)?,
    })
}

/// Helper function to decode a value. It can be a key or subkey.
pub(crate) fn decode_one<T>(value: Cow<'_, [u8]>) -> Result<T::Value, DatabaseError>
where
    T: Table,
{
    Ok(match value {
        Cow::Borrowed(v) => Decompress::decompress(v)?,
        Cow::Owned(v) => Decompress::decompress_owned(v)?,
    })
}
