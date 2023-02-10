//! Consensus for Clique PoA networks.
use super::snapshot::Snapshot;
use crate::validation::{clique, validate_block_transaction_root};
use reth_interfaces::consensus::{CliqueError, Consensus, Error, ForkchoiceState};
use reth_primitives::{ChainSpec, CliqueConfig, SealedBlock, SealedHeader, EMPTY_OMMER_ROOT, U256};
use tokio::sync::watch;

/// Implementation of Clique proof-of-authority consensus protocol.
/// https://eips.ethereum.org/EIPS/eip-225
#[derive(Debug)]
pub struct CliqueConsensus {
    /// The chain specification.
    chain_spec: ChainSpec,
    /// Cloned clique config from the chain spec for ease of access.
    config: CliqueConfig,
    /// Current voting snapshot.
    snapshot: Snapshot,
}

impl CliqueConsensus {
    /// Create new instance of clique consensus.
    pub fn new(chain_spec: ChainSpec) -> Result<Self, CliqueError> {
        let config = chain_spec.clique.clone().ok_or(CliqueError::Config)?;
        let snapshot = Snapshot::new(&chain_spec)?;
        Ok(Self { chain_spec, config, snapshot })
    }
}

impl Consensus for CliqueConsensus {
    fn fork_choice_state(&self) -> watch::Receiver<ForkchoiceState> {
        todo!()
    }

    fn pre_validate_header(
        &self,
        header: &SealedHeader,
        parent: &SealedHeader,
    ) -> Result<(), Error> {
        clique::validate_header_standalone(header, &self.config)?;
        clique::validate_header_regarding_parent(parent, header, &self.config, &self.chain_spec)?;
        Ok(())
    }

    fn validate_header(&self, header: &SealedHeader, _total_difficulty: U256) -> Result<(), Error> {
        clique::validate_header_regarding_snapshot(header, &self.snapshot, &self.config)
    }

    fn pre_validate_block(&self, block: &SealedBlock) -> Result<(), Error> {
        // Ensure that the block doesn't contain any uncles which are meaningless in PoA
        if block.ommers_hash != EMPTY_OMMER_ROOT {
            return Err(Error::BodyOmmersHashDiff {
                got: block.ommers_hash,
                expected: EMPTY_OMMER_ROOT,
            })
        }

        // Ensure that the block has a valid transaction root.
        validate_block_transaction_root(block)?;

        Ok(())
    }

    fn has_block_reward(&self, _total_difficulty: U256) -> bool {
        // No block rewards in PoA, so the state remains as is and uncles are dropped
        false
    }
}
