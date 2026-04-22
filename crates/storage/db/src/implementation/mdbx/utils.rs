//! Small database table utilities and helper functions.

use crate::{
    table::{Decode, Decompress, Table, TableRow},
    DatabaseError,
};
use std::{
    borrow::Cow,
    time::{Duration, Instant},
};

/// Helper function to decode a `(key, value)` pair.
pub(crate) fn decoder<'a, T>(
    (k, v): (Cow<'a, [u8]>, Cow<'a, [u8]>),
) -> Result<TableRow<T>, DatabaseError>
where
    T: Table,
    T::Key: Decode,
    T::Value: Decompress,
{
    Ok((
        match k {
            Cow::Borrowed(k) => Decode::decode(k)?,
            Cow::Owned(k) => Decode::decode_owned(k)?,
        },
        match v {
            Cow::Borrowed(v) => Decompress::decompress(v)?,
            Cow::Owned(v) => Decompress::decompress_owned(v)?,
        },
    ))
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

/// Applies an artificial read delay.
///
/// This uses a short busy-wait instead of `thread::sleep` so hidden benchmark knobs like
/// `--db.read-delay=500us` can approximate sub-millisecond latency without depending on OS sleep
/// granularity.
pub(crate) fn apply_read_delay(read_delay: Option<Duration>) {
    let Some(read_delay) = read_delay.filter(|delay| !delay.is_zero()) else { return };

    let start = Instant::now();
    while start.elapsed() < read_delay {
        std::hint::spin_loop();
    }
}
