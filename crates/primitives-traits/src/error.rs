use alloc::boxed::Box;
use core::ops::{Deref, DerefMut};

/// A pair of values, one of which is expected and one of which is actual.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, thiserror::Error)]
#[error("got {got}, expected {expected}")]
pub struct GotExpected<T> {
    /// The actual value.
    pub got: T,
    /// The expected value.
    pub expected: T,
}

impl<T> From<(T, T)> for GotExpected<T> {
    #[inline]
    fn from((got, expected): (T, T)) -> Self {
        Self::new(got, expected)
    }
}

impl<T> GotExpected<T> {
    /// Creates a new error from a pair of values.
    #[inline]
    pub const fn new(got: T, expected: T) -> Self {
        Self { got, expected }
    }
}

/// A pair of values, one of which is expected and one of which is actual.
///
/// Same as [`GotExpected`], but [`Box`]ed for smaller size.
///
/// Prefer instantiating using [`GotExpected`], and then using `.into()` to convert to this type.
#[derive(Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash, thiserror::Error, Debug)]
#[error(transparent)]
pub struct GotExpectedBoxed<T>(pub Box<GotExpected<T>>);

impl<T> Deref for GotExpectedBoxed<T> {
    type Target = GotExpected<T>;

    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> DerefMut for GotExpectedBoxed<T> {
    #[inline(always)]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<T> From<(T, T)> for GotExpectedBoxed<T> {
    #[inline]
    fn from(value: (T, T)) -> Self {
        Self(Box::new(GotExpected::from(value)))
    }
}

impl<T> From<GotExpected<T>> for GotExpectedBoxed<T> {
    #[inline]
    fn from(value: GotExpected<T>) -> Self {
        Self(Box::new(value))
    }
}
