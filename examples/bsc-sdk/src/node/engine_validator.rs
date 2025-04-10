use crate::{chainspec::BscChainSpec, hardforks::BscHardforks};
use alloy_consensus::{Header, EMPTY_OMMER_ROOT_HASH};
use alloy_eips::Decodable2718;
use alloy_primitives::U256;
use alloy_rpc_types_engine::{ExecutionData, ExecutionPayloadV1, PayloadAttributes, PayloadError};
use bytes::BufMut;
use reth::{
    api::{FullNodeComponents, NodeTypes},
    builder::{rpc::EngineValidatorBuilder, AddOnsContext},
    consensus::ConsensusError,
};
use reth_engine_primitives::{EngineValidator, PayloadValidator};
use reth_ethereum_primitives::Block;
use reth_node_ethereum::EthEngineTypes;
use reth_payload_primitives::{
    EngineApiMessageVersion, EngineObjectValidationError, NewPayloadError, PayloadOrAttributes,
    PayloadTypes,
};
use reth_primitives::{EthPrimitives, RecoveredBlock, SealedBlock};
use reth_primitives_traits::Block as _;
use reth_trie_common::HashedPostState;
use std::sync::Arc;

#[derive(Debug, Default, Clone)]
#[non_exhaustive]
pub struct BscEngineValidatorBuilder;

impl<Node, Types> EngineValidatorBuilder<Node> for BscEngineValidatorBuilder
where
    Types:
        NodeTypes<ChainSpec = BscChainSpec, Payload = EthEngineTypes, Primitives = EthPrimitives>,
    Node: FullNodeComponents<Types = Types>,
{
    type Validator = BscEngineValidator;

    async fn build(self, ctx: &AddOnsContext<'_, Node>) -> eyre::Result<Self::Validator> {
        Ok(BscEngineValidator::new(Arc::new(ctx.config.chain.clone().as_ref().clone().into())))
    }
}

/// Validator for Optimism engine API.
#[derive(Debug, Clone)]
pub struct BscEngineValidator {
    inner: BscExecutionPayloadValidator<BscChainSpec>,
}

impl BscEngineValidator {
    /// Instantiates a new validator.
    pub fn new(chain_spec: Arc<BscChainSpec>) -> Self {
        Self { inner: BscExecutionPayloadValidator { inner: chain_spec } }
    }

    /// Returns the chain spec used by the validator.
    #[inline]
    fn chain_spec(&self) -> &BscChainSpec {
        self.inner.chain_spec()
    }
}

impl PayloadValidator for BscEngineValidator {
    type Block = Block;
    type ExecutionData = ExecutionData;

    fn ensure_well_formed_payload(
        &self,
        payload: Self::ExecutionData,
    ) -> Result<RecoveredBlock<Self::Block>, NewPayloadError> {
        let sealed_block =
            self.inner.ensure_well_formed_payload(payload).map_err(NewPayloadError::other)?;
        sealed_block.try_recover().map_err(|e| NewPayloadError::Other(e.into()))
    }

    fn validate_block_post_execution_with_hashed_state(
        &self,
        _state_updates: &HashedPostState,
        _block: &RecoveredBlock<Self::Block>,
    ) -> Result<(), ConsensusError> {
        Ok(())
    }
}

impl<Types> EngineValidator<Types> for BscEngineValidator
where
    Types: PayloadTypes<PayloadAttributes = PayloadAttributes, ExecutionData = ExecutionData>,
{
    fn validate_version_specific_fields(
        &self,
        _version: EngineApiMessageVersion,
        _payload_or_attrs: PayloadOrAttributes<'_, Self::ExecutionData, PayloadAttributes>,
    ) -> Result<(), EngineObjectValidationError> {
        Ok(())
    }

    fn ensure_well_formed_attributes(
        &self,
        _version: EngineApiMessageVersion,
        _attributes: &PayloadAttributes,
    ) -> Result<(), EngineObjectValidationError> {
        Ok(())
    }
}

/// Execution payload validator.
#[derive(Clone, Debug)]
pub struct BscExecutionPayloadValidator<ChainSpec> {
    /// Chain spec to validate against.
    inner: Arc<ChainSpec>,
}

impl<ChainSpec> BscExecutionPayloadValidator<ChainSpec>
where
    ChainSpec: BscHardforks,
{
    /// Returns reference to chain spec.
    pub fn chain_spec(&self) -> &ChainSpec {
        &self.inner
    }

    pub fn try_into_block<T: Decodable2718>(
        &self,
        payload: ExecutionPayloadV1,
    ) -> Result<alloy_consensus::Block<T>, PayloadError> {
        let transactions = payload
            .transactions
            .iter()
            .map(|tx| {
                let mut buf = tx.as_ref();

                let tx = T::decode_2718(&mut buf).map_err(alloy_rlp::Error::from)?;

                if !buf.is_empty() {
                    return Err(alloy_rlp::Error::UnexpectedLength);
                }

                Ok(tx)
            })
            .collect::<Result<Vec<_>, _>>()?;

        // Reuse the encoded bytes for root calculation
        let transactions_root = alloy_consensus::proofs::ordered_trie_root_with_encoder(
            &payload.transactions,
            |item, buf| buf.put_slice(item),
        );

        let header = Header {
            parent_hash: payload.parent_hash,
            beneficiary: payload.fee_recipient,
            state_root: payload.state_root,
            transactions_root,
            receipts_root: payload.receipts_root,
            withdrawals_root: None,
            logs_bloom: payload.logs_bloom,
            number: payload.block_number,
            gas_limit: payload.gas_limit,
            gas_used: payload.gas_used,
            timestamp: payload.timestamp,
            mix_hash: payload.prev_randao,
            // WARNING: It’s allowed for a base fee in EIP1559 to increase unbounded. We assume that
            // it will fit in an u64. This is not always necessarily true, although it is extremely
            // unlikely not to be the case, a u64 maximum would have 2^64 which equates to 18 ETH
            // per gas.
            base_fee_per_gas: None,
            blob_gas_used: None,
            excess_blob_gas: None,
            parent_beacon_block_root: None,
            requests_hash: None,
            extra_data: payload.extra_data,
            // Defaults
            ommers_hash: EMPTY_OMMER_ROOT_HASH,
            difficulty: U256::from(2),
            nonce: Default::default(),
        };

        Ok(alloy_consensus::Block {
            header,
            body: alloy_consensus::BlockBody { transactions, ommers: vec![], withdrawals: None },
        })
    }

    pub fn ensure_well_formed_payload(
        &self,
        payload: ExecutionData,
    ) -> Result<SealedBlock<Block>, PayloadError> {
        let ExecutionData { payload, sidecar: _ } = payload;

        let expected_hash = payload.block_hash();

        let payload_v1 = payload.as_v1();

        // First parse the block
        let sealed_block = self.try_into_block(payload_v1.clone())?.seal_slow();

        // Ensure the hash included in the payload matches the block hash
        if expected_hash != sealed_block.hash() {
            return Err(PayloadError::BlockHash {
                execution: sealed_block.hash(),
                consensus: expected_hash,
            })?
        }

        Ok(sealed_block)
    }
}
