use crate::{Consensus, ConsensusError, PostExecutionInput};
use alloy_primitives::U256;
use core::sync::atomic::{AtomicBool, Ordering};
use reth_primitives::{BlockWithSenders, SealedBlock, SealedHeader};

/// Consensus engine implementation for testing
#[derive(Debug)]
pub struct TestConsensus {
    /// Flag whether the header validation should purposefully fail
    fail_validation: AtomicBool,
}

impl Default for TestConsensus {
    fn default() -> Self {
        Self { fail_validation: AtomicBool::new(false) }
    }
}

impl TestConsensus {
    /// Get the failed validation flag.
    pub fn fail_validation(&self) -> bool {
        self.fail_validation.load(Ordering::SeqCst)
    }

    /// Update the validation flag.
    pub fn set_fail_validation(&self, val: bool) {
        self.fail_validation.store(val, Ordering::SeqCst)
    }
}

impl<H, B> Consensus<H, B> for TestConsensus {
    fn validate_header(&self, _header: &SealedHeader<H>) -> Result<(), ConsensusError> {
        if self.fail_validation() {
            Err(ConsensusError::BaseFeeMissing)
        } else {
            Ok(())
        }
    }

    fn validate_header_against_parent(
        &self,
        _header: &SealedHeader<H>,
        _parent: &SealedHeader<H>,
    ) -> Result<(), ConsensusError> {
        if self.fail_validation() {
            Err(ConsensusError::BaseFeeMissing)
        } else {
            Ok(())
        }
    }

    fn validate_header_with_total_difficulty(
        &self,
        _header: &H,
        _total_difficulty: U256,
    ) -> Result<(), ConsensusError> {
        if self.fail_validation() {
            Err(ConsensusError::BaseFeeMissing)
        } else {
            Ok(())
        }
    }

    fn validate_body_against_header(
        &self,
        _body: &B,
        _header: &SealedHeader<H>,
    ) -> Result<(), ConsensusError> {
        if self.fail_validation() {
            Err(ConsensusError::BaseFeeMissing)
        } else {
            Ok(())
        }
    }

    fn validate_block_pre_execution(
        &self,
        _block: &SealedBlock<H, B>,
    ) -> Result<(), ConsensusError> {
        if self.fail_validation() {
            Err(ConsensusError::BaseFeeMissing)
        } else {
            Ok(())
        }
    }

    fn validate_block_post_execution(
        &self,
        _block: &BlockWithSenders,
        _input: PostExecutionInput<'_>,
    ) -> Result<(), ConsensusError> {
        if self.fail_validation() {
            Err(ConsensusError::BaseFeeMissing)
        } else {
            Ok(())
        }
    }
}
