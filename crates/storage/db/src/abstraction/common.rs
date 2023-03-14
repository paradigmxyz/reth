/// Alias type containing key value pairs.
pub type KeyValue<T> = (<T as Table>::Key, <T as Table>::Value);
/// Alias type for a `(key, value)` result coming from a cursor.
pub type PairResult<T> = Result<Option<KeyValue<T>>, Error>;
/// Alias type for a `(key, value)` result coming from an iterator.
pub type IterPairResult<T> = Option<Result<KeyValue<T>, Error>>;
/// Alias type for a value result coming from a cursor without its key.
pub type ValueOnlyResult<T> = Result<Option<<T as Table>::Value>, Error>;

use crate::{abstraction::table::*, Error};

// Sealed trait helper to prevent misuse of the API.
mod sealed {
    pub trait Sealed: Sized {}
    pub struct Bounds<T>(T);
    impl<T> Sealed for Bounds<T> {}
}
pub(crate) use sealed::{Bounds, Sealed};
