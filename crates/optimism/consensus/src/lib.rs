//! Optimism Consensus implementation.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
// The `optimism` feature must be enabled to use this crate.
#![cfg(feature = "optimism")]

use alloy_primitives::{B64, U256};
use reth_chainspec::EthereumHardforks;
use reth_consensus::{Consensus, ConsensusError, PostExecutionInput};
use reth_consensus_common::validation::{
    validate_against_parent_4844, validate_against_parent_eip1559_base_fee,
    validate_against_parent_hash_number, validate_against_parent_timestamp, validate_cancun_gas,
    validate_header_base_fee, validate_header_extradata, validate_header_gas,
    validate_shanghai_withdrawals,
};
use reth_optimism_chainspec::OpChainSpec;
use reth_optimism_forks::OptimismHardforks;
use reth_primitives::{
    BlockWithSenders, GotExpected, Header, SealedBlock, SealedHeader, EMPTY_OMMER_ROOT_HASH,
};
use std::{sync::Arc, time::SystemTime};

mod proof;
pub use proof::calculate_receipt_root_no_memo_optimism;

mod validation;
pub use validation::validate_block_post_execution;

/// Optimism consensus implementation.
///
/// Provides basic checks as outlined in the execution specs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptimismBeaconConsensus {
    /// Configuration
    chain_spec: Arc<OpChainSpec>,
}

impl OptimismBeaconConsensus {
    /// Create a new instance of [`OptimismBeaconConsensus`]
    pub const fn new(chain_spec: Arc<OpChainSpec>) -> Self {
        Self { chain_spec }
    }
}

impl Consensus for OptimismBeaconConsensus {
    fn validate_header(&self, header: &SealedHeader) -> Result<(), ConsensusError> {
        validate_header_gas(header)?;
        validate_header_base_fee(header, &self.chain_spec)
    }

    fn validate_header_against_parent(
        &self,
        header: &SealedHeader,
        parent: &SealedHeader,
    ) -> Result<(), ConsensusError> {
        validate_against_parent_hash_number(header, parent)?;

        if self.chain_spec.is_bedrock_active_at_block(header.number) {
            validate_against_parent_timestamp(header, parent)?;
        }

        validate_against_parent_eip1559_base_fee(header, parent, &self.chain_spec)?;

        // ensure that the blob gas fields for this block
        if self.chain_spec.is_cancun_active_at_timestamp(header.timestamp) {
            validate_against_parent_4844(header, parent)?;
        }

        Ok(())
    }

    fn validate_header_with_total_difficulty(
        &self,
        header: &Header,
        _total_difficulty: U256,
    ) -> Result<(), ConsensusError> {
        // with OP-stack Bedrock activation number determines when TTD (eth Merge) has been reached.
        let is_post_merge = self.chain_spec.is_bedrock_active_at_block(header.number);

        if is_post_merge {
            if header.nonce != B64::ZERO {
                return Err(ConsensusError::TheMergeNonceIsNotZero)
            }

            if header.ommers_hash != EMPTY_OMMER_ROOT_HASH {
                return Err(ConsensusError::TheMergeOmmerRootIsNotEmpty)
            }

            // Post-merge, the consensus layer is expected to perform checks such that the block
            // timestamp is a function of the slot. This is different from pre-merge, where blocks
            // are only allowed to be in the future (compared to the system's clock) by a certain
            // threshold.
            //
            // Block validation with respect to the parent should ensure that the block timestamp
            // is greater than its parent timestamp.

            // validate header extradata for all networks post merge
            validate_header_extradata(header)?;

            // mixHash is used instead of difficulty inside EVM
            // https://eips.ethereum.org/EIPS/eip-4399#using-mixhash-field-instead-of-difficulty
        } else {
            // Check if timestamp is in the future. Clock can drift but this can be consensus issue.
            let present_timestamp =
                SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs();

            if header.exceeds_allowed_future_timestamp(present_timestamp) {
                return Err(ConsensusError::TimestampIsInFuture {
                    timestamp: header.timestamp,
                    present_timestamp,
                })
            }
        }

        Ok(())
    }

    fn validate_block_pre_execution(&self, block: &SealedBlock) -> Result<(), ConsensusError> {
        // Check ommers hash
        let ommers_hash = reth_primitives::proofs::calculate_ommers_root(&block.body.ommers);
        if block.header.ommers_hash != ommers_hash {
            return Err(ConsensusError::BodyOmmersHashDiff(
                GotExpected { got: ommers_hash, expected: block.header.ommers_hash }.into(),
            ))
        }

        // Check transaction root
        if let Err(error) = block.ensure_transaction_root_valid() {
            return Err(ConsensusError::BodyTransactionRootDiff(error.into()))
        }

        // EIP-4895: Beacon chain push withdrawals as operations
        if self.chain_spec.is_shanghai_active_at_timestamp(block.timestamp) {
            validate_shanghai_withdrawals(block)?;
        }

        if self.chain_spec.is_cancun_active_at_timestamp(block.timestamp) {
            validate_cancun_gas(block)?;
        }

        Ok(())
    }

    fn validate_block_post_execution(
        &self,
        block: &BlockWithSenders,
        input: PostExecutionInput<'_>,
    ) -> Result<(), ConsensusError> {
        validate_block_post_execution(block, &self.chain_spec, input.receipts)
    }
}
