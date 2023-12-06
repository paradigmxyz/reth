//! [serde] utilities.

pub use reth_rpc_types::serde_helpers::*;

mod prune;
pub use prune::deserialize_opt_prune_mode_with_min_blocks;
