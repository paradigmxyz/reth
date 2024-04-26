use crate::{Consensus, ConsensusError};
use reth_primitives::{Header, SealedBlock, SealedHeader, U256};
use std::sync::atomic::{AtomicBool, Ordering};

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

impl Consensus for TestConsensus {
    fn validate_header(&self, _header: &SealedHeader) -> Result<(), ConsensusError> {
        if self.fail_validation() {
            Err(ConsensusError::BaseFeeMissing)
        } else {
            Ok(())
        }
    }

    fn validate_header_against_parent(
        &self,
        _header: &SealedHeader,
        _parent: &SealedHeader,
    ) -> Result<(), ConsensusError> {
        if self.fail_validation() {
            Err(ConsensusError::BaseFeeMissing)
        } else {
            Ok(())
        }
    }

    fn validate_header_with_total_difficulty(
        &self,
        _header: &Header,
        _total_difficulty: U256,
    ) -> Result<(), ConsensusError> {
        if self.fail_validation() {
            Err(ConsensusError::BaseFeeMissing)
        } else {
            Ok(())
        }
    }

    fn validate_block(&self, _block: &SealedBlock) -> Result<(), ConsensusError> {
        if self.fail_validation() {
            Err(ConsensusError::BaseFeeMissing)
        } else {
            Ok(())
        }
    }
}
