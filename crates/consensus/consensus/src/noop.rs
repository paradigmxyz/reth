use crate::{Consensus, ConsensusError, FullConsensus, HeaderValidator, PostExecutionInput};
use alloy_primitives::U256;
use reth_primitives::{BlockWithSenders, NodePrimitives, SealedBlock, SealedHeader};

/// A Consensus implementation that does nothing.
#[derive(Debug, Copy, Clone, Default)]
#[non_exhaustive]
pub struct NoopConsensus;

impl<H> HeaderValidator<H> for NoopConsensus {
    fn validate_header(&self, _header: &SealedHeader<H>) -> Result<(), ConsensusError> {
        Ok(())
    }

    fn validate_header_against_parent(
        &self,
        _header: &SealedHeader<H>,
        _parent: &SealedHeader<H>,
    ) -> Result<(), ConsensusError> {
        Ok(())
    }

    fn validate_header_with_total_difficulty(
        &self,
        _header: &H,
        _total_difficulty: U256,
    ) -> Result<(), ConsensusError> {
        Ok(())
    }
}

impl<H, B> Consensus<H, B> for NoopConsensus {
    fn validate_body_against_header(
        &self,
        _body: &B,
        _header: &SealedHeader<H>,
    ) -> Result<(), ConsensusError> {
        Ok(())
    }

    fn validate_block_pre_execution(
        &self,
        _block: &SealedBlock<H, B>,
    ) -> Result<(), ConsensusError> {
        Ok(())
    }
}

impl<N: NodePrimitives> FullConsensus<N> for NoopConsensus {
    fn validate_block_post_execution(
        &self,
        _block: &BlockWithSenders<N::Block>,
        _input: PostExecutionInput<'_, N::Receipt>,
    ) -> Result<(), ConsensusError> {
        Ok(())
    }
}
