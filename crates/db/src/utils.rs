//! Utils crate for `db`.

use crate::kv::{
    table::{Decode, Table},
    KVError,
};
use bytes::Bytes;
use std::borrow::Cow;

/// Enum for the type of table present in libmdbx.
#[derive(Debug)]
pub enum TableType {
    Table,
    DupSort,
}

/// Returns the default page size that can be used in this OS.
pub(crate) fn default_page_size() -> usize {
    let os_page_size = page_size::get();

    // source: https://gitflic.ru/project/erthink/libmdbx/blob?file=mdbx.h#line-num-821
    let libmdbx_max_page_size = 0x10000;

    // May lead to errors if it's reduced further because of the potential size of the
    // data.
    let min_page_size = 4096;

    os_page_size.clamp(min_page_size, libmdbx_max_page_size)
}

/// Helper function to decode a `(key, value)` pair.
pub(crate) fn decoder<'a, T>(
    kv: (Cow<'a, [u8]>, Cow<'a, [u8]>),
) -> Result<(T::Key, T::Value), KVError>
where
    T: Table,
    T::Key: Decode,
{
    Ok((
        Decode::decode(Bytes::from(kv.0.into_owned()))?,
        Decode::decode(Bytes::from(kv.1.into_owned()))?,
    ))
}

/// Helper function to decode only a value from a `(key, value)` pair.
pub(crate) fn decode_value<'a, T>(kv: (Cow<'a, [u8]>, Cow<'a, [u8]>)) -> Result<T::Value, KVError>
where
    T: Table,
{
    Decode::decode(Bytes::from(kv.1.into_owned()))
}

/// Helper function to decode a value. It can be a key or subkey.
pub(crate) fn decode_one<T>(value: Cow<'_, [u8]>) -> Result<T::Value, KVError>
where
    T: Table,
{
    Decode::decode(Bytes::from(value.into_owned()))
}
