use crate::{chainspec::BscChainSpec, hardforks::BscHardforks};
use alloy_primitives::U256;
use reth::{
    api::{FullNodeTypes, NodeTypes},
    builder::{components::ConsensusBuilder, BuilderContext},
    consensus::{Consensus, ConsensusError, FullConsensus, HeaderValidator},
};
use reth_chainspec::EthChainSpec;
use reth_primitives::{EthPrimitives, NodePrimitives, RecoveredBlock, SealedBlock, SealedHeader};
use reth_primitives_traits::{Block, BlockHeader};
use reth_provider::BlockExecutionResult;
use std::sync::Arc;

/// A basic Bsc consensus builder.
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct BscConsensusBuilder;

impl<Node> ConsensusBuilder<Node> for BscConsensusBuilder
where
    Node: FullNodeTypes<Types: NodeTypes<ChainSpec = BscChainSpec, Primitives = EthPrimitives>>,
{
    type Consensus = Arc<dyn FullConsensus<EthPrimitives, Error = ConsensusError>>;

    async fn build_consensus(self, ctx: &BuilderContext<Node>) -> eyre::Result<Self::Consensus> {
        Ok(Arc::new(BscConsensus::new(ctx.chain_spec())))
    }
}

/// BSC consensus implementation.
///
/// Provides basic checks as outlined in the execution specs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BscConsensus<ChainSpec> {
    chain_spec: Arc<ChainSpec>,
}

impl<ChainSpec> BscConsensus<ChainSpec> {
    /// Create a new instance of [`BscConsensus`]
    pub const fn new(chain_spec: Arc<ChainSpec>) -> Self {
        Self { chain_spec }
    }
}

impl<ChainSpec: EthChainSpec + BscHardforks, N: NodePrimitives> FullConsensus<N>
    for BscConsensus<ChainSpec>
{
    fn validate_block_post_execution(
        &self,
        _block: &RecoveredBlock<N::Block>,
        _result: &BlockExecutionResult<N::Receipt>,
    ) -> Result<(), ConsensusError> {
        Ok(())
    }
}

impl<ChainSpec: EthChainSpec + BscHardforks, B: Block> Consensus<B> for BscConsensus<ChainSpec> {
    type Error = ConsensusError;

    fn validate_body_against_header(
        &self,
        _body: &B::Body,
        _header: &SealedHeader<B::Header>,
    ) -> Result<(), ConsensusError> {
        // validate_body_against_header(body, header.header())
        Ok(())
    }

    fn validate_block_pre_execution(&self, _block: &SealedBlock<B>) -> Result<(), ConsensusError> {
        // Check ommers hash
        // let ommers_hash = block.body().calculate_ommers_root();
        // if Some(block.ommers_hash()) != ommers_hash {
        //     return Err(ConsensusError::BodyOmmersHashDiff(
        //         GotExpected {
        //             got: ommers_hash.unwrap_or(EMPTY_OMMER_ROOT_HASH),
        //             expected: block.ommers_hash(),
        //         }
        //         .into(),
        //     ))
        // }

        // // Check transaction root
        // if let Err(error) = block.ensure_transaction_root_valid() {
        //     return Err(ConsensusError::BodyTransactionRootDiff(error.into()))
        // }

        // if self.chain_spec.is_cancun_active_at_timestamp(block.timestamp()) {
        //     validate_cancun_gas(block)?;
        // } else {
        //     return Ok(())
        // }

        Ok(())
    }
}

impl<ChainSpec: EthChainSpec + BscHardforks, H: BlockHeader> HeaderValidator<H>
    for BscConsensus<ChainSpec>
{
    fn validate_header(&self, _header: &SealedHeader<H>) -> Result<(), ConsensusError> {
        // validate_header_gas(header.header())?;
        // validate_header_base_fee(header.header(), &self.chain_spec)
        Ok(())
    }

    fn validate_header_against_parent(
        &self,
        _header: &SealedHeader<H>,
        _parent: &SealedHeader<H>,
    ) -> Result<(), ConsensusError> {
        // validate_against_parent_hash_number(header.header(), parent)?;

        // validate_against_parent_eip1559_base_fee(
        //     header.header(),
        //     parent.header(),
        //     &self.chain_spec,
        // )?;

        // // ensure that the blob gas fields for this block
        // if let Some(blob_params) = self.chain_spec.blob_params_at_timestamp(header.timestamp()) {
        //     validate_against_parent_4844(header.header(), parent.header(), blob_params)?;
        // }

        Ok(())
    }

    fn validate_header_with_total_difficulty(
        &self,
        _header: &H,
        _total_difficulty: U256,
    ) -> Result<(), ConsensusError> {
        // if header.nonce() != Some(B64::ZERO) {
        //     return Err(ConsensusError::TheMergeNonceIsNotZero)
        // }

        // if header.ommers_hash() != EMPTY_OMMER_ROOT_HASH {
        //     return Err(ConsensusError::TheMergeOmmerRootIsNotEmpty)
        // }

        Ok(())
    }
}
