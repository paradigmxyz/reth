//! Consensus for ethereum network
use reth_consensus_common::validation;
use reth_interfaces::consensus::{Consensus, ConsensusError};
use reth_primitives::{
    constants::{ALLOWED_FUTURE_BLOCK_TIME_SECONDS, MAXIMUM_EXTRA_DATA_SIZE},
    Chain, ChainSpec, Hardfork, Header, SealedBlock, SealedHeader, EMPTY_OMMER_ROOT_HASH, U256,
};
use std::{sync::Arc, time::SystemTime};

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
            // TODO Consensus checks for old blocks:
            //  * difficulty, mix_hash & nonce aka PoW stuff
            // low priority as syncing is done in reverse order

            // Check if timestamp is in future. Clock can drift but this can be consensus issue.
            let present_timestamp =
                SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs();

            if header.timestamp > present_timestamp + ALLOWED_FUTURE_BLOCK_TIME_SECONDS {
                return Err(ConsensusError::TimestampIsInFuture {
                    timestamp: header.timestamp,
                    present_timestamp,
                })
            }

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
    if header.extra_data.len() > MAXIMUM_EXTRA_DATA_SIZE {
        Err(ConsensusError::ExtraDataExceedsMax { len: header.extra_data.len() })
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_primitives::constants::MAXIMUM_EXTRA_DATA_SIZE;

    fn create_test_chain_spec() -> Arc<ChainSpec> {
        Arc::new(ChainSpec::default())
    }

    fn create_test_sealed_header() -> SealedHeader {
        SealedHeader::default()
    }

    #[test]
    fn test_validate_header_with_valid_data() {
        let chain_spec = create_test_chain_spec();
        let beacon_consensus = BeaconConsensus::new(chain_spec);
        let sealed_header = create_test_sealed_header();

        assert!(beacon_consensus.validate_header(&sealed_header).is_ok());
    }

    #[test]
    fn test_validate_header_against_parent_with_valid_data() {
        let chain_spec = create_test_chain_spec();
        let beacon_consensus = BeaconConsensus::new(chain_spec);
        let parent = create_test_sealed_header();

        let mut header = create_test_sealed_header();
        header.header.number = 1;

        header.header.parent_hash = parent.header.hash_slow();

        assert!(beacon_consensus.validate_header_against_parent(&header, &parent).is_ok());
    }

    #[test]
    fn test_validate_header_extradata_with_valid_length() {
        let header =
            Header { extra_data: vec![0; MAXIMUM_EXTRA_DATA_SIZE].into(), ..Default::default() };

        assert!(validate_header_extradata(&header).is_ok());
    }

    #[test]
    fn test_validate_header_extradata_with_excessive_length() {
        let header = Header {
            extra_data: vec![0; MAXIMUM_EXTRA_DATA_SIZE + 1].into(),
            ..Default::default()
        };

        assert!(matches!(
            validate_header_extradata(&header),
            Err(ConsensusError::ExtraDataExceedsMax { len }) if len == MAXIMUM_EXTRA_DATA_SIZE + 1
        ));
    }
}
