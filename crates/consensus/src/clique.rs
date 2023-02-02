//! Consensus for Clique PoA networks.
use reth_interfaces::consensus::{Consensus, Error, ForkchoiceState};
use reth_primitives::{BlockNumber, SealedBlock, SealedHeader};
use tokio::sync::watch;

#[derive(Debug)]
pub struct CliqueConsensus {}

impl Consensus for CliqueConsensus {
    fn fork_choice_state(&self) -> watch::Receiver<ForkchoiceState> {
        todo!()
    }

    fn validate_header(&self, header: &SealedHeader, parent: &SealedHeader) -> Result<(), Error> {
        todo!()
    }

    fn pre_validate_block(&self, block: &SealedBlock) -> Result<(), Error> {
        todo!()
    }

    fn has_block_reward(&self, block_num: BlockNumber) -> bool {
        todo!()
    }
}
