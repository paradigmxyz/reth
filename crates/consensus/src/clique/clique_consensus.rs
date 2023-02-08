//! Consensus for Clique PoA networks.
use super::snapshot::Snapshot;
use crate::validation::clique;
use reth_interfaces::consensus::{Consensus, Error, ForkchoiceState};
use reth_primitives::{BlockNumber, ChainSpec, CliqueConfig, SealedBlock, SealedHeader};
use tokio::sync::watch;

/// Implementation of Clique proof-of-authority consensus protocol.
/// https://eips.ethereum.org/EIPS/eip-225
#[derive(Debug)]
pub struct CliqueConsensus {
    chain_spec: ChainSpec,
    /// Cloned clique config from the chain spec for ease of access.
    config: CliqueConfig,
}

impl CliqueConsensus {
    /// Create new instance of clique consensus.
    pub fn new(chain_spec: ChainSpec) -> Self {
        let config = chain_spec.clique.clone().expect("clique config exists");
        Self { chain_spec, config }
    }

    fn retrieve_snapshot(&self) -> Snapshot {
        todo!()
    }
}

impl Consensus for CliqueConsensus {
    fn fork_choice_state(&self) -> watch::Receiver<ForkchoiceState> {
        todo!()
    }

    fn validate_header(&self, header: &SealedHeader, parent: &SealedHeader) -> Result<(), Error> {
        clique::validate_header_standalone(header, &self.config)?;
        clique::validate_header_regarding_parent(parent, header, &self.config, &self.chain_spec)?;

        // TODO: Retrieve the snapshot
        let snapshot = self.retrieve_snapshot();
        clique::validate_header_regarding_snapshot(header, &snapshot, &self.config)?;

        Ok(())
    }

    fn pre_validate_block(&self, block: &SealedBlock) -> Result<(), Error> {
        todo!()
    }

    fn has_block_reward(&self, block_num: BlockNumber) -> bool {
        todo!()
    }
}
