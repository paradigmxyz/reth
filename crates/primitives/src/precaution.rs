//! Helpers to ensure certain features are enabled or disabled.
//!
//! The motivation for this is to prevent that a binary is accidentally built with a feature that is
//! not intended to be used.
//!
//! Currently conflicting features are: `value-u256` which is required by optimism.

/// A macro to ensure that the crate's features are compatible with ethereum
#[macro_export]
macro_rules! ensure_ethereum {
    () => {
        #[cfg(feature = "value-256")]
        compile_error!("The `value-256` feature is enabled but for `ethereum` it must be disabled: https://github.com/paradigmxyz/reth/issues/4891");
    };
}

/// A macro to ensure that the crate's features are compatible with optimism
#[macro_export]
macro_rules! ensure_optimism {
    () => {
        #[cfg(not(feature = "value-256"))]
        compile_error!("The `value-256` feature is disabled but for `optimism` it must be enabled: https://github.com/paradigmxyz/reth/issues/4891");
    };
}
