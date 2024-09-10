mod sealed;
pub use sealed::SealedHeader;

mod error;
pub use error::HeaderError;

#[cfg(any(test, feature = "test-utils", feature = "arbitrary"))]
pub mod test_utils;

pub use alloy_consensus::Header;
