use crate::kv::table::{Decode, Table};
use std::borrow::Cow;

/// Returns the default page size that can be used in this OS.
pub fn default_page_size() -> usize {
    let os_page_size = page_size::get();
    let libmdbx_max_page_size = 0x10000;

    if os_page_size < 4096 {
        // may lead to errors if it's reduced further because of the potential size of the data
        4096
    } else if os_page_size > libmdbx_max_page_size {
        libmdbx_max_page_size
    } else {
        os_page_size
    }
}

/// Helper function to decode a `(key, value)` pair.
pub fn decoder<'a, T>(kv: (Cow<'a, [u8]>, Cow<'a, [u8]>)) -> eyre::Result<(T::Key, T::Value)>
where
    T: Table,
    T::Key: Decode,
{
    Ok((Decode::decode(&kv.0)?, Decode::decode(&kv.1)?))
}

/// Helper function to decode only a value from a `(key, value)` pair.
pub fn decode_value<'a, T>(kv: (Cow<'a, [u8]>, Cow<'a, [u8]>)) -> eyre::Result<T::Value>
where
    T: Table,
{
    Decode::decode(&kv.1)
}

/// Helper function to decode a value. It can be a key or subkey.
pub fn decode_one<'a, T>(value: Cow<'a, [u8]>) -> eyre::Result<T::Value>
where
    T: Table,
{
    Decode::decode(&value)
}
