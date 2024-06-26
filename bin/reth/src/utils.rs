//! Common CLI utility functions.

/// Exposing `open_db_read_only` function
pub mod db {
    pub use reth_db::open_db_read_only;
}

/// Re-exported from `reth_node_core`, also to prevent a breaking change. See the comment on
/// the `reth_node_core::args` re-export for more details.
pub use reth_node_core::utils::*;
