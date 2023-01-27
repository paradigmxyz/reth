//! Consensus for ethereum network
use crate::verification;
use reth_interfaces::consensus::{Consensus, Error, ForkchoiceState};
use reth_primitives::{
    BlockNumber, ChainSpec, ForkDiscriminant, Hardfork, SealedBlock, SealedHeader, H256,
};
use tokio::sync::{watch, watch::error::SendError};

/// Ethereum beacon consensus
///
/// This consensus engine does basic checks as outlined in the execution specs,
/// but otherwise defers consensus on what the current chain is to a consensus client.
#[derive(Debug)]
pub struct BeaconConsensus {
    /// Watcher over the forkchoice state
    channel: (watch::Sender<ForkchoiceState>, watch::Receiver<ForkchoiceState>),
    /// Configuration
    chain_spec: ChainSpec,
}

impl BeaconConsensus {
    /// Create a new instance of [BeaconConsensus]
    pub fn new(chain_spec: ChainSpec) -> Self {
        Self {
            channel: watch::channel(ForkchoiceState {
                head_block_hash: H256::zero(),
                finalized_block_hash: H256::zero(),
                safe_block_hash: H256::zero(),
            }),
            chain_spec,
        }
    }
}

impl Consensus for BeaconConsensus {
    fn fork_choice_state(&self) -> watch::Receiver<ForkchoiceState> {
        self.channel.1.clone()
    }

    fn notify_fork_choice_state(
        &self,
        state: ForkchoiceState,
    ) -> Result<(), SendError<ForkchoiceState>> {
        self.channel.0.send(state)
    }

    fn validate_header(&self, header: &SealedHeader, parent: &SealedHeader) -> Result<(), Error> {
        verification::validate_header_standalone(header, &self.chain_spec)?;
        verification::validate_header_regarding_parent(parent, header, &self.chain_spec)?;

        if !self.chain_spec.fork_active(
            Hardfork::Paris,
            ForkDiscriminant::ttd(
                self.chain_spec.terminal_total_difficulty().unwrap_or_default(),
                Some(header.number),
            ),
        ) {
            // TODO Consensus checks for old blocks:
            //  * difficulty, mix_hash & nonce aka PoW stuff
            // low priority as syncing is done in reverse order
        }
        Ok(())
    }

    fn pre_validate_block(&self, block: &SealedBlock) -> Result<(), Error> {
        verification::validate_block_standalone(block)
    }

    fn has_block_reward(&self, block_num: BlockNumber) -> bool {
        !self.chain_spec.fork_active(
            Hardfork::Paris,
            ForkDiscriminant::ttd(
                self.chain_spec.terminal_total_difficulty().unwrap_or_default(),
                Some(block_num),
            ),
        )
    }
}
