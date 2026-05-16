//! Helper errors.

use alloc::{borrow::Cow, boxed::Box, string::ToString};
use core::{convert::Infallible, fmt::Display};

/// Helper type that is [`core::error::Error`] and wraps a value and an error message.
///
/// This can be used to return an object as part of an `Err` and is used for fallible conversions.
#[derive(Debug, thiserror::Error)]
#[error("{msg}")]
pub struct ValueError<T> {
    msg: Cow<'static, str>,
    value: Box<T>,
}

impl<T> ValueError<T> {
    /// Creates a new error with the given value and error message.
    pub fn new(value: T, msg: impl Display) -> Self {
        Self { msg: Cow::Owned(msg.to_string()), value: Box::new(value) }
    }

    /// Creates a new error with a static error message.
    pub fn new_static(value: T, msg: &'static str) -> Self {
        Self { msg: Cow::Borrowed(msg), value: Box::new(value) }
    }

    /// Converts the value to the given alternative that is `From<T>`.
    pub fn convert<U>(self) -> ValueError<U>
    where
        U: From<T>,
    {
        self.map(U::from)
    }

    /// Maps the error's value with the given closure.
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> ValueError<U> {
        ValueError { msg: self.msg, value: Box::new(f(*self.value)) }
    }

    /// Consumes the type and returns the underlying value.
    pub fn into_value(self) -> T {
        *self.value
    }

    /// Returns a reference to the value.
    pub const fn value(&self) -> &T {
        &self.value
    }
}

/// The error for conversions or processing of transactions of type using components that lack the
/// knowledge or capability to do so.
#[derive(Debug, thiserror::Error)]
#[error("Unsupported transaction type: {0}")]
pub struct UnsupportedTransactionType<TxType: Display>(TxType);

impl<TxType: Display> UnsupportedTransactionType<TxType> {
    /// Creates new `UnsupportedTransactionType` showing `ty` as the unsupported type.
    pub const fn new(ty: TxType) -> Self {
        Self(ty)
    }

    /// Converts the `UnsupportedTransactionType` into the `TxType` it contains, taking self.
    pub fn into_inner(self) -> TxType {
        self.0
    }
}

impl<TxType: Display> AsRef<TxType> for UnsupportedTransactionType<TxType> {
    fn as_ref(&self) -> &TxType {
        &self.0
    }
}

impl<TxType: Display> From<Infallible> for UnsupportedTransactionType<TxType> {
    fn from(value: Infallible) -> Self {
        match value {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TxType;

    #[test]
    fn test_unsupported_tx_type_error_displays_itself_and_the_type() {
        let error = UnsupportedTransactionType::new(TxType::Eip2930);
        let actual_msg = error.to_string();
        let expected_msg = "Unsupported transaction type: EIP-2930";

        assert_eq!(actual_msg, expected_msg);
    }
}
