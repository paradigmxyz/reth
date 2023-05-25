//! Various assertion helpers.

use crate::Error;
use std::fmt::Debug;

/// A helper like `assert_eq!` that instead returns `Err(Error::Assertion)` on failure.
pub fn assert_equal<T>(left: T, right: T, msg: &str) -> Result<(), Error>
where
    T: Eq + Debug,
{
    if left != right {
        return Err(Error::Assertion(format!("{msg}. Left {:?}, right {:?}", left, right)))
    }

    Ok(())
}
