use crate::{Consensus, ConsensusError, FullConsensus, HeaderValidator};
use alloc::sync::Arc;
use alloy_primitives::U256;
use reth_execution_types::BlockExecutionResult;
use reth_primitives_traits::{Block, NodePrimitives, RecoveredBlock, SealedBlock, SealedHeader};

/// A Consensus implementation that does nothing.
#[derive(Debug, Copy, Clone, Default)]
#[non_exhaustive]
pub struct NoopConsensus;

impl NoopConsensus {
    /// Creates an Arc instance of Self.
    pub fn arc() -> Arc<Self> {
        Arc::new(Self::default())
    }
}

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

impl<B: Block> Consensus<B> for NoopConsensus {
    type Error = ConsensusError;

    fn validate_body_against_header(
        &self,
        _body: &B::Body,
        _header: &SealedHeader<B::Header>,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    fn validate_block_pre_execution(&self, _block: &SealedBlock<B>) -> Result<(), Self::Error> {
        Ok(())
    }
}

impl<N: NodePrimitives> FullConsensus<N> for NoopConsensus {
    fn validate_block_post_execution(
        &self,
        _block: &RecoveredBlock<N::Block>,
        _result: &BlockExecutionResult<N::Receipt>,
    ) -> Result<(), ConsensusError> {
        Ok(())
    }
}
