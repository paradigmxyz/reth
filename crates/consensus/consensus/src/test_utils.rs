use crate::{Consensus, ConsensusError, FullConsensus, HeaderValidator, PostExecutionInput};
use alloy_primitives::U256;
use core::sync::atomic::{AtomicBool, Ordering};
use reth_primitives::{BlockWithSenders, NodePrimitives, SealedBlock, SealedHeader};

/// Consensus engine implementation for testing
#[derive(Debug)]
pub struct TestConsensus {
    /// Flag whether the header validation should purposefully fail
    fail_validation: AtomicBool,
    /// Separate flag for setting whether `validate_body_against_header` should fail. It is needed
    /// for testing networking logic for which the body failing this check is getting completely
    /// rejected while more high-level failures are handled by the sync logic.
    fail_body_against_header: AtomicBool,
}

impl Default for TestConsensus {
    fn default() -> Self {
        Self {
            fail_validation: AtomicBool::new(false),
            fail_body_against_header: AtomicBool::new(false),
        }
    }
}

impl TestConsensus {
    /// Get the failed validation flag.
    pub fn fail_validation(&self) -> bool {
        self.fail_validation.load(Ordering::SeqCst)
    }

    /// Update the validation flag.
    pub fn set_fail_validation(&self, val: bool) {
        self.fail_validation.store(val, Ordering::SeqCst);
        self.fail_body_against_header.store(val, Ordering::SeqCst);
    }

    /// Returns the body validation flag.
    pub fn fail_body_against_header(&self) -> bool {
        self.fail_body_against_header.load(Ordering::SeqCst)
    }

    /// Update the body validation flag.
    pub fn set_fail_body_against_header(&self, val: bool) {
        self.fail_body_against_header.store(val, Ordering::SeqCst);
    }
}

impl<N: NodePrimitives> FullConsensus<N> for TestConsensus {
    fn validate_block_post_execution(
        &self,
        _block: &BlockWithSenders<N::Block>,
        _input: PostExecutionInput<'_, N::Receipt>,
    ) -> Result<(), ConsensusError> {
        if self.fail_validation() {
            Err(ConsensusError::BaseFeeMissing)
        } else {
            Ok(())
        }
    }
}

impl<H, B> Consensus<H, B> for TestConsensus {
    type Error = ConsensusError;

    fn validate_body_against_header(
        &self,
        _body: &B,
        _header: &SealedHeader<H>,
    ) -> Result<(), Self::Error> {
        if self.fail_body_against_header() {
            Err(ConsensusError::BaseFeeMissing)
        } else {
            Ok(())
        }
    }

    fn validate_block_pre_execution(&self, _block: &SealedBlock<H, B>) -> Result<(), Self::Error> {
        if self.fail_validation() {
            Err(ConsensusError::BaseFeeMissing)
        } else {
            Ok(())
        }
    }
}

impl<H> HeaderValidator<H> for TestConsensus {
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
}
