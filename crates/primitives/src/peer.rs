// Re-export PeerId for ease of use.
pub use reth_rpc_types::PeerId;

/// Generic wrapper with peer id
#[derive(Debug)]
pub struct WithPeerId<T>(PeerId, pub T);

impl<T> From<(PeerId, T)> for WithPeerId<T> {
    fn from(value: (PeerId, T)) -> Self {
        Self(value.0, value.1)
    }
}

impl<T> WithPeerId<T> {
    /// Wraps the value with the peerid.
    pub fn new(peer: PeerId, value: T) -> Self {
        Self(peer, value)
    }

    /// Get the peer id
    pub fn peer_id(&self) -> PeerId {
        self.0
    }

    /// Get the underlying data
    pub fn data(&self) -> &T {
        &self.1
    }

    /// Returns ownership of the underlying data.
    pub fn into_data(self) -> T {
        self.1
    }

    /// Transform the data
    pub fn transform<F: From<T>>(self) -> WithPeerId<F> {
        WithPeerId(self.0, self.1.into())
    }

    /// Split the wrapper into [PeerId] and data tuple
    pub fn split(self) -> (PeerId, T) {
        (self.0, self.1)
    }

    /// Maps the inner value to a new value using the given function.
    pub fn map<U, F: FnOnce(T) -> U>(self, op: F) -> WithPeerId<U> {
        WithPeerId(self.0, op(self.1))
    }
}

impl<T> WithPeerId<Option<T>> {
    /// returns `None` if the inner value is `None`, otherwise returns `Some(WithPeerId<T>)`.
    pub fn transpose(self) -> Option<WithPeerId<T>> {
        self.1.map(|v| WithPeerId(self.0, v))
    }
}
