mod sealed;
pub use sealed::{BlockWithParent, SealedHeader};

mod error;
pub use error::HeaderError;

#[cfg(any(test, feature = "test-utils", feature = "arbitrary"))]
pub mod test_utils;

use alloy_consensus::Header;
use alloy_primitives::{Address, BlockNumber, B256, U256};

/// Bincode-compatible header type serde implementations.
#[cfg(feature = "serde-bincode-compat")]
pub mod serde_bincode_compat {
    pub use super::sealed::serde_bincode_compat::SealedHeader;
}
