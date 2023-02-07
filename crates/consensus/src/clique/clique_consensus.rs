//! Consensus for Clique PoA networks.
use super::{constants::*, snapshot::Snapshot};
use crate::validation::clique;
use reth_interfaces::consensus::{CliqueError, Consensus, Error, ForkchoiceState};
use reth_primitives::{
    BlockNumber, Bytes, ChainSpec, CliqueConfig, Header, SealedBlock, SealedHeader,
};
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
    fn seal_header(&self, mut header: Header) -> Result<SealedHeader, Error> {
        let extra_data = std::mem::take(&mut header.extra_data);
        let extra_data_len = extra_data.len();
        let end_byte = extra_data_len
            .checked_sub(EXTRA_SEAL)
            .ok_or(CliqueError::MissingSignature { extra_data: extra_data.clone() })?;

        // Set trimmed extra data on header.
        header.extra_data = Bytes::from(&extra_data[..end_byte]);
        let hash = header.hash_slow();

        // Reset the `extra_data` field
        header.extra_data = extra_data;
        Ok(header.seal(hash))
    }

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

#[cfg(test)]
mod tests {
    use super::*;
    use reth_primitives::{hex_literal::hex, ChainSpecBuilder, H256};

    // Check that header hash calculation is correct.
    // https://github.com/ethereum/go-ethereum/blob/d0a4989a8def7e6bad182d1513e8d4a093c1672d/consensus/clique/clique_test.go#L114-L125
    #[test]
    fn test_seal_header() {
        let expected =
            H256(hex!("bd3d1fa43fbc4c5bfcc91b179ec92e2861df3654de60468beb908ff805359e8f"));
        let consensus = CliqueConsensus::new(ChainSpecBuilder::mainnet().build());

        let extra_data = Bytes::from(vec![0; EXTRA_VANITY + EXTRA_SEAL]);
        let header = Header {
            ommers_hash: H256::zero(),
            state_root: H256::zero(),
            transactions_root: H256::zero(),
            receipts_root: H256::zero(),
            base_fee_per_gas: Some(0),
            extra_data: extra_data.clone(),
            ..Default::default()
        };

        let sealed = consensus.seal_header(header).unwrap();
        assert_eq!(sealed.hash(), expected);
        assert_eq!(sealed.extra_data, extra_data);
    }
}
