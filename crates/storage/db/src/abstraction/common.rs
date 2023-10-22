/// A key-value pair for table `T`.
pub type KeyValue<T> = (<T as Table>::Key, <T as Table>::Value);

/// A fallible key-value pair that may or may not exist.
///
/// The `Result` represents that the operation might fail, while the `Option` represents whether or
/// not the entry exists.
pub type PairResult<T> = Result<Option<KeyValue<T>>, DatabaseError>;

/// A key-value pair coming from an iterator.
///
/// The `Result` represents that the operation might fail, while the `Option` represents whether or
/// not there is another entry.
pub type IterPairResult<T> = Option<Result<KeyValue<T>, DatabaseError>>;

/// A value only result for table `T`.
pub type ValueOnlyResult<T> = Result<Option<<T as Table>::Value>, DatabaseError>;

use crate::{abstraction::table::*, DatabaseError};

// Sealed trait helper to prevent misuse of the API.
mod sealed {
    pub trait Sealed: Sized {}
    #[allow(missing_debug_implementations)]
    pub struct Bounds<T>(T);
    impl<T> Sealed for Bounds<T> {}
}
pub(crate) use sealed::{Bounds, Sealed};
