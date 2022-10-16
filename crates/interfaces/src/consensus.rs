use async_trait::async_trait;
use reth_primitives::{Header, H256};
use reth_rpc_types::engine::ForkchoiceState;
use std::fmt::Debug;
use thiserror::Error;
use tokio::sync::watch::Receiver;

/// Consensus is a protocol that chooses canonical chain.
/// We are checking validity of block header here.
#[async_trait]
pub trait Consensus: Sync + Send + Debug {
    /// Get a receiver for the fork choice state
    fn forkchoice_state(&self) -> Receiver<ForkchoiceState>;

    /// Return the current chain tip
    fn tip(&self) -> H256;

    /// Validate if header is correct and follows consensus specification
    fn validate_header(&self, _header: &Header, _parent: &Header) -> Result<(), Error> {
        Ok(())
    }
}

/// Consensus errors (TODO)
#[derive(Error, Debug)]
pub enum Error {
    /// Explanatory
    #[error("Example of consensus error")]
    ConsensusError,
}
