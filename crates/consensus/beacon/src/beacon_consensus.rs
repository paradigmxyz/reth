//! Consensus for ethereum network
use reth_consensus_common::validation;
use reth_interfaces::consensus::{Consensus, ConsensusError};
use reth_primitives::{
    Chain, ChainSpec, Hardfork, Header, SealedBlock, SealedHeader, EMPTY_OMMER_ROOT, U256,
};
use std::sync::Arc;

/// Ethereum beacon consensus
///
/// This consensus engine does basic checks as outlined in the execution specs.
#[derive(Debug)]
pub struct BeaconConsensus {
    /// Configuration
    chain_spec: Arc<ChainSpec>,
}

impl BeaconConsensus {
    /// Create a new instance of [BeaconConsensus]
    pub fn new(chain_spec: Arc<ChainSpec>) -> Self {
        Self { chain_spec }
    }
}

impl Consensus for BeaconConsensus {
    fn validate_header(&self, header: &SealedHeader) -> Result<(), ConsensusError> {
        validation::validate_header_standalone(header, &self.chain_spec)?;
        Ok(())
    }

    fn validate_header_against_parent(
        &self,
        header: &SealedHeader,
        parent: &SealedHeader,
    ) -> Result<(), ConsensusError> {
        validation::validate_header_regarding_parent(parent, header, &self.chain_spec)?;
        Ok(())
    }

    fn validate_header_with_total_difficulty(
        &self,
        header: &Header,
        total_difficulty: U256,
    ) -> Result<(), ConsensusError> {
        if self.chain_spec.fork(Hardfork::Paris).active_at_ttd(total_difficulty, header.difficulty)
        {
            // EIP-3675: Upgrade consensus to Proof-of-Stake:
            // https://eips.ethereum.org/EIPS/eip-3675#replacing-difficulty-with-0
            if header.difficulty != U256::ZERO {
                return Err(ConsensusError::TheMergeDifficultyIsNotZero)
            }

            if header.nonce != 0 {
                return Err(ConsensusError::TheMergeNonceIsNotZero)
            }

            if header.ommers_hash != EMPTY_OMMER_ROOT {
                return Err(ConsensusError::TheMergeOmmerRootIsNotEmpty)
            }

            // validate header extradata for all networks post merge
            validate_header_extradata(header)?;

            // mixHash is used instead of difficulty inside EVM
            // https://eips.ethereum.org/EIPS/eip-4399#using-mixhash-field-instead-of-difficulty
        } else {
            // TODO Consensus checks for old blocks:
            //  * difficulty, mix_hash & nonce aka PoW stuff
            // low priority as syncing is done in reverse order

            // Goerli exception:
            //  * If the network is goerli pre-merge, ignore the extradata check, since we do not
            //  support clique.
            if self.chain_spec.chain != Chain::goerli() {
                validate_header_extradata(header)?;
            }
        }

        Ok(())
    }

    fn validate_block(&self, block: &SealedBlock) -> Result<(), ConsensusError> {
        validation::validate_block_standalone(block, &self.chain_spec)
    }
}

/// Validates the header's extradata according to the beacon consensus rules.
///
/// From yellow paper: extraData: An arbitrary byte array containing data relevant to this block.
/// This must be 32 bytes or fewer; formally Hx.
fn validate_header_extradata(header: &Header) -> Result<(), ConsensusError> {
    if header.extra_data.len() > 32 {
        Err(ConsensusError::ExtraDataExceedsMax { len: header.extra_data.len() })
    } else {
        Ok(())
    }
}
