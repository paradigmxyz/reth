use crate::{Consensus, ConsensusError, PostExecutionInput};
use alloy_primitives::U256;
use reth_primitives::{BlockWithSenders, Header, SealedBlock, SealedHeader};

/// A Consensus implementation that does nothing.
#[derive(Debug, Copy, Clone, Default)]
#[non_exhaustive]
pub struct NoopConsensus;

impl Consensus for NoopConsensus {
    fn validate_header(&self, _header: &SealedHeader) -> Result<(), ConsensusError> {
        Ok(())
    }

    fn validate_header_against_parent(
        &self,
        _header: &SealedHeader,
        _parent: &SealedHeader,
    ) -> Result<(), ConsensusError> {
        Ok(())
    }

    fn validate_header_with_total_difficulty(
        &self,
        _header: &Header,
        _total_difficulty: U256,
    ) -> Result<(), ConsensusError> {
        Ok(())
    }

    fn validate_block_pre_execution(&self, _block: &SealedBlock) -> Result<(), ConsensusError> {
        Ok(())
    }

    fn validate_block_post_execution(
        &self,
        _block: &BlockWithSenders,
        _input: PostExecutionInput<'_>,
    ) -> Result<(), ConsensusError> {
        Ok(())
    }
}
