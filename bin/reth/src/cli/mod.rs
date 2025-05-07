//! CLI definition and entrypoint to executable

/// Re-export of the [`reth_node_core`] types specifically in the `cli` module.
///
/// This is re-exported because the types in `reth_node_core::cli` originally existed in
/// `reth::cli` but were moved to the [`reth_node_core`] crate. This re-export avoids a
/// breaking change.
pub use crate::core::cli::*;

/// Re-export of the [`reth_ethereum_cli`] types specifically in the `interface` module.
///
/// This is re-exported because the types in [`reth_ethereum_cli::interface`] originally
/// existed in `reth::cli` but were moved to the [`reth_ethereum_cli`] crate. This re-export
/// avoids a breaking change.
pub use reth_ethereum_cli::interface::*;
