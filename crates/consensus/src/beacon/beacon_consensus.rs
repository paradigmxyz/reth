//! Consensus for ethereum network
use crate::validation;
use reth_interfaces::consensus::{Consensus, ConsensusError, ForkchoiceState};
use reth_primitives::{ChainSpec, Hardfork, SealedBlock, SealedHeader, EMPTY_OMMER_ROOT, U256};
use tokio::sync::watch;

use super::BeaconConsensusBuilder;

/// Ethereum beacon consensus
///
/// This consensus engine does basic checks as outlined in the execution specs,
/// but otherwise defers consensus on what the current chain is to a consensus client.
#[derive(Debug)]
pub struct BeaconConsensus {
    /// Watcher over the forkchoice state
    forkchoice_state_rx: watch::Receiver<ForkchoiceState>,
    /// Configuration
    chain_spec: ChainSpec,
}

impl BeaconConsensus {
    /// Create a new instance of [BeaconConsensus]
    pub fn new(
        chain_spec: ChainSpec,
        forkchoice_state_rx: watch::Receiver<ForkchoiceState>,
    ) -> Self {
        Self { chain_spec, forkchoice_state_rx }
    }

    /// Create new [BeaconConsensusBuilder].
    pub fn builder() -> BeaconConsensusBuilder {
        BeaconConsensusBuilder::default()
    }
}

impl Consensus for BeaconConsensus {
    fn fork_choice_state(&self) -> watch::Receiver<ForkchoiceState> {
        self.forkchoice_state_rx.clone()
    }

    fn pre_validate_header(
        &self,
        header: &SealedHeader,
        parent: &SealedHeader,
    ) -> Result<(), ConsensusError> {
        validation::validate_header_standalone(header, &self.chain_spec)?;
        validation::validate_header_regarding_parent(parent, header, &self.chain_spec)?;

        Ok(())
    }

    fn validate_header(
        &self,
        header: &SealedHeader,
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

            // mixHash is used instead of difficulty inside EVM
            // https://eips.ethereum.org/EIPS/eip-4399#using-mixhash-field-instead-of-difficulty
        } else {
            // TODO Consensus checks for old blocks:
            //  * difficulty, mix_hash & nonce aka PoW stuff
            // low priority as syncing is done in reverse order
        }

        Ok(())
    }

    fn pre_validate_block(&self, block: &SealedBlock) -> Result<(), ConsensusError> {
        validation::validate_block_standalone(block, &self.chain_spec)
    }

    fn has_block_reward(&self, total_difficulty: U256, difficulty: U256) -> bool {
        !self.chain_spec.fork(Hardfork::Paris).active_at_ttd(total_difficulty, difficulty)
    }
}

#[cfg(test)]
mod test {
    use reth_interfaces::consensus::Consensus;
    use reth_primitives::{ChainSpecBuilder, U256};

    use super::BeaconConsensus;

    #[test]
    fn test_has_block_reward_before_paris() {
        let chain_spec = ChainSpecBuilder::mainnet().build();
        let (consensus, _) = BeaconConsensus::builder().build(chain_spec);
        assert!(consensus.has_block_reward(U256::ZERO, U256::ZERO));
    }
}
