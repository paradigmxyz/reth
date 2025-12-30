mod sealed;
pub use sealed::{Header, SealedHeader, SealedHeaderFor};

mod header_mut;
pub use header_mut::HeaderMut;

#[cfg(any(test, feature = "test-utils", feature = "arbitrary"))]
pub mod test_utils;

/// Bincode-compatible header type serde implementations.
#[cfg(feature = "serde-bincode-compat")]
pub mod serde_bincode_compat {
    pub use super::sealed::serde_bincode_compat::SealedHeader;
}
