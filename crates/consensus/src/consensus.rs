//! Consensus for ethereum network

use crate::{verification, Config};
use reth_interfaces::consensus::{Consensus, Error, ForkchoiceState};
use reth_primitives::{BlockLocked, BlockNumber, SealedHeader, H256};
use tokio::sync::{watch, watch::error::SendError};

/// Ethereum consensus
pub struct EthConsensus {
    /// Watcher over the forkchoice state
    channel: (watch::Sender<ForkchoiceState>, watch::Receiver<ForkchoiceState>),
    /// Configuration
    config: Config,
}

impl EthConsensus {
    /// Create a new instance of [EthConsensus]
    pub fn new(config: Config) -> Self {
        Self {
            channel: watch::channel(ForkchoiceState {
                head_block_hash: H256::zero(),
                finalized_block_hash: H256::zero(),
                safe_block_hash: H256::zero(),
            }),
            config,
        }
    }

    /// Notifies all listeners of the latest [ForkchoiceState].
    pub fn notify_fork_choice_state(
        &self,
        state: ForkchoiceState,
    ) -> Result<(), SendError<ForkchoiceState>> {
        self.channel.0.send(state)
    }
}

impl Consensus for EthConsensus {
    fn fork_choice_state(&self) -> watch::Receiver<ForkchoiceState> {
        self.channel.1.clone()
    }

    fn validate_header(&self, header: &SealedHeader, parent: &SealedHeader) -> Result<(), Error> {
        verification::validate_header_standalone(header, &self.config)?;
        verification::validate_header_regarding_parent(parent, header, &self.config)?;

        if header.number < self.config.paris_block {
            // TODO Consensus checks for old blocks:
            //  * difficulty, mix_hash & nonce aka PoW stuff
            // low priority as syncing is done in reverse order
        }
        Ok(())
    }

    fn pre_validate_block(&self, block: &BlockLocked) -> Result<(), Error> {
        verification::validate_block_standalone(block)
    }

    fn has_block_reward(&self, block_num: BlockNumber) -> bool {
        block_num <= self.config.paris_block
    }
}
