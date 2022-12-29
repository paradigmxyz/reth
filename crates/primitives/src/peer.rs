use ethers_core::types::H512;

// TODO: should we use `PublicKey` for this? Even when dealing with public keys we should try to
// prevent misuse
/// This represents an uncompressed secp256k1 public key.
/// This encodes the concatenation of the x and y components of the affine point in bytes.
pub type PeerId = H512;

/// Generic wrapper with peer id
#[derive(Debug)]
pub struct WithPeerId<T>(PeerId, pub T);

impl<T> From<(H512, T)> for WithPeerId<T> {
    fn from(value: (H512, T)) -> Self {
        Self(value.0, value.1)
    }
}

impl<T> WithPeerId<T> {
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
}
