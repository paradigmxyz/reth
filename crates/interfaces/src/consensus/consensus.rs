use super::Error;
use async_trait::async_trait;
use reth_primitives::{SealedBlock, SealedHeader, U256};
use reth_rpc_types::engine::ForkchoiceState;
use std::fmt::Debug;
use tokio::sync::watch::Receiver;

/// Consensus is a protocol that chooses canonical chain.
#[async_trait]
#[auto_impl::auto_impl(&, Arc)]
pub trait Consensus: Debug + Send + Sync {
    /// Get a receiver for the fork choice state
    fn fork_choice_state(&self) -> Receiver<ForkchoiceState>;

    /// Validate if the header is correct and follows consensus specification.
    ///
    /// This is called before properties that are not in the header itself (like total difficulty)
    /// have been computed.
    ///
    /// **This should not be called for the genesis block**.
    fn pre_validate_header(
        &self,
        header: &SealedHeader,
        parent: &SealedHeader,
    ) -> Result<(), Error>;

    /// Validate if the header is correct and follows the consensus specification, including
    /// computed properties (like total difficulty).
    ///
    /// Some consensus engines may want to do additional checks here.
    fn validate_header(&self, header: &SealedHeader, total_difficulty: U256) -> Result<(), Error>;

    /// Validate a block disregarding world state, i.e. things that can be checked before sender
    /// recovery and execution.
    ///
    /// See the Yellow Paper sections 4.3.2 "Holistic Validity", 4.3.4 "Block Header Validity", and
    /// 11.1 "Ommer Validation".
    ///
    /// **This should not be called for the genesis block**.
    fn pre_validate_block(&self, block: &SealedBlock) -> Result<(), Error>;

    /// After the Merge (aka Paris) block rewards became obsolete.
    ///
    /// This flag is needed as reth's changeset is indexed on transaction level granularity.
    ///
    /// More info [here](https://github.com/paradigmxyz/reth/issues/237)
    fn has_block_reward(&self, total_difficulty: U256) -> bool;
}
