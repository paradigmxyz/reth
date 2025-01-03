//! Optimism Consensus implementation.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
// The `optimism` feature must be enabled to use this crate.
#![cfg(feature = "optimism")]

use alloy_consensus::{BlockHeader, Header, EMPTY_OMMER_ROOT_HASH};
use alloy_eips::eip7840::BlobParams;
use alloy_primitives::{B64, U256};
use reth_chainspec::EthereumHardforks;
use reth_consensus::{
    Consensus, ConsensusError, FullConsensus, HeaderValidator, PostExecutionInput,
};
use reth_consensus_common::validation::{
    validate_against_parent_4844, validate_against_parent_eip1559_base_fee,
    validate_against_parent_hash_number, validate_against_parent_timestamp,
    validate_body_against_header, validate_cancun_gas, validate_header_base_fee,
    validate_header_extra_data, validate_header_gas, validate_shanghai_withdrawals,
};
use reth_optimism_chainspec::OpChainSpec;
use reth_optimism_forks::OpHardforks;
use reth_optimism_primitives::{OpBlock, OpBlockBody, OpPrimitives, OpReceipt};
use reth_primitives::{BlockWithSenders, GotExpected, SealedBlockFor, SealedHeader};
use std::{sync::Arc, time::SystemTime};

mod proof;
pub use proof::calculate_receipt_root_no_memo_optimism;

mod validation;
pub use validation::validate_block_post_execution;

/// Optimism consensus implementation.
///
/// Provides basic checks as outlined in the execution specs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpBeaconConsensus {
    /// Configuration
    chain_spec: Arc<OpChainSpec>,
}

impl OpBeaconConsensus {
    /// Create a new instance of [`OpBeaconConsensus`]
    pub const fn new(chain_spec: Arc<OpChainSpec>) -> Self {
        Self { chain_spec }
    }
}

impl FullConsensus<OpPrimitives> for OpBeaconConsensus {
    fn validate_block_post_execution(
        &self,
        block: &BlockWithSenders<OpBlock>,
        input: PostExecutionInput<'_, OpReceipt>,
    ) -> Result<(), ConsensusError> {
        validate_block_post_execution(block, &self.chain_spec, input.receipts)
    }
}

impl Consensus<Header, OpBlockBody> for OpBeaconConsensus {
    fn validate_body_against_header(
        &self,
        body: &OpBlockBody,
        header: &SealedHeader,
    ) -> Result<(), ConsensusError> {
        validate_body_against_header(body, header.header())
    }

    fn validate_block_pre_execution(
        &self,
        block: &SealedBlockFor<OpBlock>,
    ) -> Result<(), ConsensusError> {
        // Check ommers hash
        let ommers_hash = block.body().calculate_ommers_root();
        if block.ommers_hash != ommers_hash {
            return Err(ConsensusError::BodyOmmersHashDiff(
                GotExpected { got: ommers_hash, expected: block.ommers_hash }.into(),
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
}

impl HeaderValidator for OpBeaconConsensus {
    fn validate_header(&self, header: &SealedHeader) -> Result<(), ConsensusError> {
        validate_header_gas(header.header())?;
        validate_header_base_fee(header.header(), &self.chain_spec)
    }

    fn validate_header_against_parent(
        &self,
        header: &SealedHeader,
        parent: &SealedHeader,
    ) -> Result<(), ConsensusError> {
        validate_against_parent_hash_number(header.header(), parent)?;

        if self.chain_spec.is_bedrock_active_at_block(header.number) {
            validate_against_parent_timestamp(header.header(), parent.header())?;
        }

        // EIP1559 base fee validation
        // <https://github.com/ethereum-optimism/specs/blob/main/specs/protocol/holocene/exec-engine.md#base-fee-computation>
        // > if Holocene is active in parent_header.timestamp, then the parameters from
        // > parent_header.extraData are used.
        if self.chain_spec.is_holocene_active_at_timestamp(parent.timestamp) {
            let header_base_fee =
                header.base_fee_per_gas().ok_or(ConsensusError::BaseFeeMissing)?;
            let expected_base_fee = self
                .chain_spec
                .decode_holocene_base_fee(parent, header.timestamp)
                .map_err(|_| ConsensusError::BaseFeeMissing)?;
            if expected_base_fee != header_base_fee {
                return Err(ConsensusError::BaseFeeDiff(GotExpected {
                    expected: expected_base_fee,
                    got: header_base_fee,
                }))
            }
        } else {
            validate_against_parent_eip1559_base_fee(
                header.header(),
                parent.header(),
                &self.chain_spec,
            )?;
        }

        // ensure that the blob gas fields for this block
        if self.chain_spec.is_cancun_active_at_timestamp(header.timestamp) {
            validate_against_parent_4844(header.header(), parent.header(), BlobParams::cancun())?;
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

            // validate header extra data for all networks post merge
            validate_header_extra_data(header)?;

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
}
