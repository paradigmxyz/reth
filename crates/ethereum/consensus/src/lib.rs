//! Beacon consensus implementation.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

use reth_consensus::{Consensus, ConsensusError};
use reth_consensus_common::validation;
use reth_primitives::{
    Chain, ChainSpec, Hardfork, Header, SealedBlock, SealedHeader, EMPTY_OMMER_ROOT_HASH, U256,
};
use std::{sync::Arc, time::SystemTime};

/// Ethereum beacon consensus
///
/// This consensus engine does basic checks as outlined in the execution specs.
#[derive(Debug)]
pub struct EthBeaconConsensus {
    /// Configuration
    chain_spec: Arc<ChainSpec>,
}

impl EthBeaconConsensus {
    /// Create a new instance of [EthBeaconConsensus]
    pub fn new(chain_spec: Arc<ChainSpec>) -> Self {
        Self { chain_spec }
    }
}

impl Consensus for EthBeaconConsensus {
    fn validate_header(&self, header: &SealedHeader) -> Result<(), ConsensusError> {
        validation::validate_header_standalone(header, &self.chain_spec)?;
        Ok(())
    }

    fn validate_header_against_parent(
        &self,
        header: &SealedHeader,
        parent: &SealedHeader,
    ) -> Result<(), ConsensusError> {
        header.validate_against_parent(parent, &self.chain_spec).map_err(ConsensusError::from)?;
        Ok(())
    }

    #[allow(unused_assignments)]
    #[allow(unused_mut)]
    fn validate_header_with_total_difficulty(
        &self,
        header: &Header,
        total_difficulty: U256,
    ) -> Result<(), ConsensusError> {
        let mut is_post_merge = self
            .chain_spec
            .fork(Hardfork::Paris)
            .active_at_ttd(total_difficulty, header.difficulty);

        #[cfg(feature = "optimism")]
        {
            // If OP-Stack then bedrock activation number determines when TTD (eth Merge) has been
            // reached.
            is_post_merge = self.chain_spec.is_bedrock_active_at_block(header.number);
        }

        if is_post_merge {
            if !self.chain_spec.is_optimism() && !header.is_zero_difficulty() {
                return Err(ConsensusError::TheMergeDifficultyIsNotZero)
            }

            if header.nonce != 0 {
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
            validation::validate_header_extradata(header)?;

            // mixHash is used instead of difficulty inside EVM
            // https://eips.ethereum.org/EIPS/eip-4399#using-mixhash-field-instead-of-difficulty
        } else {
            // TODO Consensus checks for old blocks:
            //  * difficulty, mix_hash & nonce aka PoW stuff
            // low priority as syncing is done in reverse order

            // Check if timestamp is in the future. Clock can drift but this can be consensus issue.
            let present_timestamp =
                SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs();

            if header.exceeds_allowed_future_timestamp(present_timestamp) {
                return Err(ConsensusError::TimestampIsInFuture {
                    timestamp: header.timestamp,
                    present_timestamp,
                })
            }

            // Goerli and early OP exception:
            //  * If the network is goerli pre-merge, ignore the extradata check, since we do not
            //  support clique. Same goes for OP blocks below Bedrock.
            if self.chain_spec.chain != Chain::goerli() && !self.chain_spec.is_optimism() {
                validation::validate_header_extradata(header)?;
            }
        }

        Ok(())
    }

    fn validate_block(&self, block: &SealedBlock) -> Result<(), ConsensusError> {
        validation::validate_block_standalone(block, &self.chain_spec)
    }
}
