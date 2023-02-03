use super::Error;
use async_trait::async_trait;
use reth_primitives::{BlockNumber, Header, SealedBlock, SealedHeader};
use reth_rpc_types::engine::ForkchoiceState;
use std::fmt::Debug;
use tokio::sync::watch::Receiver;

/// Consensus is a protocol that chooses canonical chain.
#[async_trait]
#[auto_impl::auto_impl(&, Arc)]
pub trait Consensus: Debug + Send + Sync {
    /// Calculate header hash and seal the Header so that it can't be changed.
    fn seal_header(&self, header: Header) -> Result<SealedHeader, Error>;

    /// Get a receiver for the fork choice state
    fn fork_choice_state(&self) -> Receiver<ForkchoiceState>;

    /// Validate if header is correct and follows consensus specification.
    ///
    /// **This should not be called for the genesis block**.
    fn validate_header(&self, header: &SealedHeader, parent: &SealedHeader) -> Result<(), Error>;

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
    fn has_block_reward(&self, block_num: BlockNumber) -> bool;
}
