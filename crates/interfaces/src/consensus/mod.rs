mod consensus;
mod error;

pub use consensus::Consensus;
pub use error::{CliqueError, Error};

/// Re-export fork choice state
pub use reth_rpc_types::engine::ForkchoiceState;
