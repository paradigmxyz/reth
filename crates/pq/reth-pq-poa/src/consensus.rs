//! `PoA` consensus validation: wraps standard Ethereum consensus with ML-DSA-65
//! seal verification on block headers.
//!
//! In `PoA` mode, each block's `extra_data` field contains the proposer's
//! ML-DSA-65 signature (3309 bytes) over `SHAKE-256(RLP(header))` where the
//! header has `extra_data` cleared to empty bytes.  This binds the seal to
//! every header field (parent hash, state root, transactions root, etc.)
//! while excluding the seal itself.
//!
//! This validator:
//! 1. Delegates standard checks (gas, timestamp, etc.) to `EthBeaconConsensus`
//! 2. Verifies the ML-DSA-65 seal against the expected round-robin proposer
//! 3. Rejects blocks without a valid seal (except genesis, block 0)

use std::sync::Arc;

use reth_consensus::{Consensus, ConsensusError, FullConsensus, HeaderValidator, ReceiptRootBloom};
use reth_execution_types::BlockExecutionResult;
use reth_primitives_traits::{Block, NodePrimitives, RecoveredBlock, SealedBlock, SealedHeader};

use crate::sealer::{header_seal_hash, verify_seal};
use crate::validator::ValidatorSet;

/// ML-DSA-65 signature length in bytes.
const SEAL_LENGTH: usize = 3309;

/// `PoA` consensus validator for `PostQuantumEVM`.
///
/// Wraps an inner consensus implementation and adds ML-DSA-65 seal
/// verification for blocks that contain a `PoA` seal in `extra_data`.
///
/// When `validator_set` is `None`, all seal checks are skipped (non-PoA mode).
#[derive(Debug, Clone)]
pub struct PqPoaConsensus<C> {
    /// Inner consensus (typically `EthBeaconConsensus`).
    inner: C,
    /// The authorized validator set for seal verification.
    /// `None` means `PoA` validation is disabled (passthrough mode).
    validator_set: Option<Arc<ValidatorSet>>,
}

impl<C> PqPoaConsensus<C> {
    /// Create a new `PoA` consensus validator with an active validator set.
    pub fn new(inner: C, validator_set: ValidatorSet) -> Self {
        Self {
            inner,
            validator_set: Some(Arc::new(validator_set)),
        }
    }

    /// Create a passthrough consensus validator (no `PoA` seal checks).
    pub const fn passthrough(inner: C) -> Self {
        Self {
            inner,
            validator_set: None,
        }
    }

    /// Verify the ML-DSA-65 seal in a block header.
    ///
    /// The seal hash is computed as `SHAKE-256(RLP(header_with_empty_extra_data))`,
    /// binding the seal to every header field while excluding the seal itself.
    ///
    /// Returns `Ok(())` if:
    /// - `PoA` is disabled (no validator set)
    /// - Block number is 0 (genesis)
    ///
    /// Returns `Err(ConsensusError)` if:
    /// - The `extra_data` length is not 3309 (missing or wrong-sized seal)
    /// - The seal signature verification fails
    /// - The seal is from a non-authorized validator
    fn verify_header_seal(
        &self,
        header: &alloy_consensus::Header,
    ) -> Result<(), ConsensusError> {
        let validator_set = match &self.validator_set {
            Some(vs) => vs,
            None => return Ok(()), // PoA disabled
        };

        // Genesis block (block 0) has no seal
        if header.number == 0 {
            return Ok(());
        }

        // Reject blocks without a valid seal
        let extra_data = header.extra_data.as_ref();
        if extra_data.len() != SEAL_LENGTH {
            return Err(ConsensusError::Other(format!(
                "PoA: block {} missing ML-DSA-65 seal (extra_data: {} bytes, expected {})",
                header.number,
                extra_data.len(),
                SEAL_LENGTH,
            )));
        }

        // Compute the seal hash: SHAKE-256(RLP(header with empty extra_data))
        let seal_hash = header_seal_hash(header);

        // Determine the expected proposer for this block
        let proposer = validator_set.proposer_at(header.number);

        // Verify the seal against the proposer's public key
        verify_seal(&proposer.public_key, &seal_hash, extra_data)
            .map_err(|e| ConsensusError::Other(format!(
                "PoA seal verification failed for block {}: {e}",
                header.number
            )))
    }
}

impl<C, B> Consensus<B> for PqPoaConsensus<C>
where
    C: Consensus<B>,
    B: Block<Header = alloy_consensus::Header>,
{
    fn validate_body_against_header(
        &self,
        body: &B::Body,
        header: &SealedHeader<B::Header>,
    ) -> Result<(), ConsensusError> {
        self.inner.validate_body_against_header(body, header)
    }

    fn validate_block_pre_execution(&self, block: &SealedBlock<B>) -> Result<(), ConsensusError> {
        // First: standard Ethereum validation (gas, timestamp, etc.)
        self.inner.validate_block_pre_execution(block)?;

        // Then: PoA seal verification over the full RLP-encoded header
        self.verify_header_seal(block.header())?;

        Ok(())
    }
}

impl<C> HeaderValidator<alloy_consensus::Header> for PqPoaConsensus<C>
where
    C: HeaderValidator<alloy_consensus::Header>,
{
    fn validate_header(
        &self,
        header: &SealedHeader<alloy_consensus::Header>,
    ) -> Result<(), ConsensusError> {
        self.inner.validate_header(header)?;

        // Also verify PoA seal during header-only validation (e.g. sync)
        self.verify_header_seal(header.header())?;

        Ok(())
    }

    fn validate_header_against_parent(
        &self,
        header: &SealedHeader<alloy_consensus::Header>,
        parent: &SealedHeader<alloy_consensus::Header>,
    ) -> Result<(), ConsensusError> {
        self.inner.validate_header_against_parent(header, parent)
    }
}

impl<C, N> FullConsensus<N> for PqPoaConsensus<C>
where
    C: FullConsensus<N>,
    N: NodePrimitives<Block: Block<Header = alloy_consensus::Header>>,
{
    fn validate_block_post_execution(
        &self,
        block: &RecoveredBlock<N::Block>,
        result: &BlockExecutionResult<N::Receipt>,
        receipt_root_bloom: Option<ReceiptRootBloom>,
    ) -> Result<(), ConsensusError> {
        self.inner
            .validate_block_post_execution(block, result, receipt_root_bloom)
    }
}
