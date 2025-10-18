use std::sync::Arc;
use core::fmt::Debug;
use alloy_consensus::{BlockHeader as _, EMPTY_OMMER_ROOT_HASH};
use alloy_primitives::B64;
use reth_chainspec::EthChainSpec;
use reth_consensus::{Consensus, ConsensusError, FullConsensus, HeaderValidator};
use reth_consensus_common::validation::{
    validate_against_parent_eip1559_base_fee,
    validate_against_parent_hash_number,
    validate_header_base_fee, validate_header_extra_data,
};
use reth_execution_types::BlockExecutionResult;
use reth_primitives_traits::{Block, BlockBody, BlockHeader, GotExpected, NodePrimitives, RecoveredBlock, SealedBlock, SealedHeader};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArbBeaconConsensus<ChainSpec> {
    chain_spec: Arc<ChainSpec>,
}

impl<ChainSpec> ArbBeaconConsensus<ChainSpec> {
    pub const fn new(chain_spec: Arc<ChainSpec>) -> Self {
        Self { chain_spec }
    }
}

impl<N, ChainSpec> FullConsensus<N> for ArbBeaconConsensus<ChainSpec>
where
    N: NodePrimitives,
    ChainSpec: EthChainSpec<Header = N::BlockHeader> + Debug + Send + Sync + reth_chainspec::EthereumHardforks,
{
    fn validate_block_post_execution(
        &self,
        _block: &RecoveredBlock<N::Block>,
        _result: &BlockExecutionResult<N::Receipt>,
    ) -> Result<(), ConsensusError> {
        Ok(())
    }
}

impl<B, ChainSpec> Consensus<B> for ArbBeaconConsensus<ChainSpec>
where
    B: Block,
    ChainSpec: EthChainSpec<Header = B::Header> + Debug + Send + Sync + reth_chainspec::EthereumHardforks,
{
    type Error = ConsensusError;

    fn validate_body_against_header(
        &self,
        body: &B::Body,
        header: &SealedHeader<B::Header>,
    ) -> Result<(), ConsensusError> {
        let ommers_hash = body.calculate_ommers_root();
        if Some(header.header().ommers_hash()) != ommers_hash {
            return Err(ConsensusError::BodyOmmersHashDiff(
                GotExpected {
                    got: ommers_hash.unwrap_or(EMPTY_OMMER_ROOT_HASH),
                    expected: header.header().ommers_hash(),
                }
                .into(),
            ));
        }
        if header.header().transactions_root() != body.calculate_tx_root() {
            return Err(ConsensusError::BodyTransactionRootDiff(
                GotExpected { got: body.calculate_tx_root(), expected: header.header().transactions_root() }.into(),
            ));
        }
        Ok(())
    }

    fn validate_block_pre_execution(&self, block: &SealedBlock<B>) -> Result<(), ConsensusError> {
        let ommers_hash = block.body().calculate_ommers_root();
        if Some(block.ommers_hash()) != ommers_hash {
            return Err(ConsensusError::BodyOmmersHashDiff(
                GotExpected {
                    got: ommers_hash.unwrap_or(EMPTY_OMMER_ROOT_HASH),
                    expected: block.ommers_hash(),
                }
                .into(),
            ));
        }
        if let Err(error) = block.ensure_transaction_root_valid() {
            return Err(ConsensusError::BodyTransactionRootDiff(error.into()));
        }
        Ok(())
    }
}

impl<H, ChainSpec> HeaderValidator<H> for ArbBeaconConsensus<ChainSpec>
where
    H: BlockHeader,
    ChainSpec: EthChainSpec<Header = H> + Debug + Send + Sync + reth_chainspec::EthereumHardforks,
{
    fn validate_header(&self, header: &SealedHeader<H>) -> Result<(), ConsensusError> {
        let h = header.header();
        if h.ommers_hash() != EMPTY_OMMER_ROOT_HASH {
            return Err(ConsensusError::TheMergeOmmerRootIsNotEmpty);
        }
        Ok(())
    }

    fn validate_header_against_parent(
        &self,
        header: &SealedHeader<H>,
        parent: &SealedHeader<H>,
    ) -> Result<(), ConsensusError> {
        let h = header.header();
        validate_against_parent_hash_number(h, parent)?;
        let parent_ts = parent.timestamp();
        let ts = h.timestamp();
        if ts < parent_ts {
            return Err(ConsensusError::TimestampIsInPast { parent_timestamp: parent_ts, timestamp: ts });
        }
        Ok(())
    }
}
