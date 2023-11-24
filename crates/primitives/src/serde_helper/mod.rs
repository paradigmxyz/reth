//! [serde] utilities.

pub use reth_rpc_types::serde_helpers::*;

mod prune;
pub use prune::deserialize_opt_prune_mode_with_min_blocks;

mod base_fee_params;
pub use base_fee_params::{deserialize_base_fee_params, serialize_base_fee_params};
