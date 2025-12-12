//! A consensus implementation that does nothing.
//!
//! This module provides `NoopConsensus`, a consensus implementation that performs no validation
//! and always returns `Ok(())` for all validation methods. Useful for testing and scenarios
//! where consensus validation is not required.
//!
//! # Examples
//!
//! ```rust
//! use reth_consensus::noop::NoopConsensus;
//! use std::sync::Arc;
//!
//! let consensus = NoopConsensus::default();
//! let consensus_arc = NoopConsensus::arc();
//! ```
//!
//! # Warning
//!
//! **Not for production use** - provides no security guarantees or consensus validation.

use crate::{Consensus, ConsensusError, FullConsensus, HeaderValidator};
use alloc::sync::Arc;
use reth_execution_types::BlockExecutionResult;
use reth_primitives_traits::{Block, NodePrimitives, RecoveredBlock, SealedBlock, SealedHeader};

/// A Consensus implementation that does nothing.
///
/// Always returns `Ok(())` for all validation methods. Suitable for testing and scenarios
/// where consensus validation is not required.
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
    /// Validates a header (no-op implementation).
    fn validate_header(&self, _header: &SealedHeader<H>) -> Result<(), ConsensusError> {
        Ok(())
    }

    /// Validates a header against its parent (no-op implementation).
    fn validate_header_against_parent(
        &self,
        _header: &SealedHeader<H>,
        _parent: &SealedHeader<H>,
    ) -> Result<(), ConsensusError> {
        Ok(())
    }
}

impl<B: Block> Consensus<B> for NoopConsensus {
    type Error = ConsensusError;

    /// Validates body against header (no-op implementation).
    fn validate_body_against_header(
        &self,
        _body: &B::Body,
        _header: &SealedHeader<B::Header>,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    /// Validates block before execution (no-op implementation).
    fn validate_block_pre_execution(&self, _block: &SealedBlock<B>) -> Result<(), Self::Error> {
        Ok(())
    }
}

impl<N: NodePrimitives> FullConsensus<N> for NoopConsensus {
    /// Validates block after execution (no-op implementation).
    fn validate_block_post_execution(
        &self,
        _block: &RecoveredBlock<N::Block>,
        _result: &BlockExecutionResult<N::Receipt>,
    ) -> Result<(), ConsensusError> {
        Ok(())
    }
}
