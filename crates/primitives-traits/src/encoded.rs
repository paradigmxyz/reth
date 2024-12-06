use alloy_eips::eip2718::Encodable2718;
use alloy_primitives::Bytes;

/// Generic wrapper with encoded Bytes, such as transaction data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WithEncoded<T>(Bytes, pub T);

impl<T> From<(Bytes, T)> for WithEncoded<T> {
    fn from(value: (Bytes, T)) -> Self {
        Self(value.0, value.1)
    }
}

impl<T> WithEncoded<T> {
    /// Wraps the value with the bytes.
    pub const fn new(bytes: Bytes, value: T) -> Self {
        Self(bytes, value)
    }

    /// Get the encoded bytes
    pub const fn encoded_bytes(&self) -> &Bytes {
        &self.0
    }

    /// Get the underlying value
    pub const fn value(&self) -> &T {
        &self.1
    }

    /// Returns ownership of the underlying value.
    pub fn into_value(self) -> T {
        self.1
    }

    /// Transform the value
    pub fn transform<F: From<T>>(self) -> WithEncoded<F> {
        WithEncoded(self.0, self.1.into())
    }

    /// Split the wrapper into [`Bytes`] and value tuple
    pub fn split(self) -> (Bytes, T) {
        (self.0, self.1)
    }

    /// Maps the inner value to a new value using the given function.
    pub fn map<U, F: FnOnce(T) -> U>(self, op: F) -> WithEncoded<U> {
        WithEncoded(self.0, op(self.1))
    }
}

impl<T: Encodable2718> WithEncoded<T> {
    /// Wraps the value with the [`Encodable2718::encoded_2718`] bytes.
    pub fn from_2718_encodable(value: T) -> Self {
        Self(value.encoded_2718().into(), value)
    }
}

impl<T> WithEncoded<Option<T>> {
    /// returns `None` if the inner value is `None`, otherwise returns `Some(WithEncoded<T>)`.
    pub fn transpose(self) -> Option<WithEncoded<T>> {
        self.1.map(|v| WithEncoded(self.0, v))
    }
}
