//! Consensus for ethereum network
use crate::validation;
use reth_interfaces::consensus::{Consensus, Error, ForkchoiceState};
use reth_primitives::{BlockNumber, ChainSpec, SealedBlock, SealedHeader};
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

    fn validate_header(&self, header: &SealedHeader, parent: &SealedHeader) -> Result<(), Error> {
        validation::validate_header_standalone(header, &self.chain_spec)?;
        validation::validate_header_regarding_parent(parent, header, &self.chain_spec)?;

        if Some(header.number) < self.chain_spec.paris_status().block_number() {
            // TODO Consensus checks for old blocks:
            //  * difficulty, mix_hash & nonce aka PoW stuff
            // low priority as syncing is done in reverse order
        }
        Ok(())
    }

    fn pre_validate_block(&self, block: &SealedBlock) -> Result<(), Error> {
        validation::validate_block_standalone(block)
    }

    fn has_block_reward(&self, block_num: BlockNumber) -> bool {
        Some(block_num) < self.chain_spec.paris_status().block_number()
    }
}
