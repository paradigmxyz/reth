//! Consensus for ethereum network

use crate::{verification, Config};
use reth_interfaces::consensus::{Consensus, Error, ForkchoiceState};
use reth_primitives::{BlockLocked, SealedHeader, H256};
use tokio::sync::watch;

/// Ethereum consensus
pub struct EthConsensus {
    /// Watcher over the forkchoice state
    channel: (watch::Sender<ForkchoiceState>, watch::Receiver<ForkchoiceState>),
    /// Configuration
    config: Config,
}

impl EthConsensus {
    /// Create new object
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
}

impl Consensus for EthConsensus {
    fn fork_choice_state(&self) -> watch::Receiver<ForkchoiceState> {
        self.channel.1.clone()
    }

    fn validate_header(&self, header: &SealedHeader, parent: &SealedHeader) -> Result<(), Error> {
        verification::validate_header_standalone(header, &self.config)?;
        verification::validate_header_regarding_parent(parent, header, &self.config)?;

        if header.number < self.config.paris_hard_fork_block {
            // TODO Consensus checks for old blocks:
            //  * difficulty, mix_hash & nonce aka PoW stuff
            // low priority as syncing is done in reverse order
        }
        Ok(())
    }

    fn pre_validate_block(&self, block: &BlockLocked) -> Result<(), Error> {
        verification::validate_block_standalone(block)
    }
}
