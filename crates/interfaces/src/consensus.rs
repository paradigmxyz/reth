use async_trait::async_trait;
use reth_primitives::Header;
use thiserror::Error;
use tokio::sync::watch::Receiver;

/// Re-export forkchoice state
pub use reth_rpc_types::engine::ForkchoiceState;

/// Consensus is a protocol that chooses canonical chain.
/// We are checking validity of block header here.
#[async_trait]
#[auto_impl::auto_impl(&, Arc)]
pub trait Consensus: Send + Sync {
    /// Get a receiver for the fork choice state
    fn fork_choice_state(&self) -> Receiver<ForkchoiceState>;

    /// Validate if header is correct and follows consensus specification
    fn validate_header(&self, header: &Header, parent: &Header) -> Result<(), Error>;
}

/// Consensus errors (TODO)
#[derive(Error, Debug)]
pub enum Error {
    /// Explanatory
    #[error("Example of consensus error")]
    ConsensusError,
}
