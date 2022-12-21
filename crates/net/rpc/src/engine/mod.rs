use crate::{result::ToRpcResult, EthApiSpec};
use async_trait::async_trait;
use jsonrpsee::core::RpcResult as Result;
use reth_consensus::Config;
use reth_interfaces::consensus::ForkchoiceState;
use reth_primitives::{
    proofs::{self, EMPTY_LIST_HASH},
    Block, BlockLocked, Header, TransactionSigned, H256, H64,
};
use reth_rlp::Decodable;
use reth_rpc_api::EngineApiServer;
use reth_rpc_types::engine::{
    ExecutionPayload, ForkchoiceUpdated, PayloadAttributes, PayloadStatus, PayloadStatusEnum,
    TransitionConfiguration,
};

mod error;
pub use error::EngineApiError;

/// The Engine API result type
pub type EngineApiResult<Ok> = std::result::Result<Ok, EngineApiError>;

/// The server implementation of Engine API
pub struct EngineApi<Eth> {
    /// The implementation of `eth` API
    eth: Eth,
    /// Consensus configuration
    config: Config,
}

impl<Eth> std::fmt::Debug for EngineApi<Eth> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineApi").field("config", &self.config).finish_non_exhaustive()
    }
}

impl<Eth> EngineApi<Eth>
where
    Eth: EthApiSpec + 'static,
{
    fn block_by_hash(&self, hash: H256) -> EngineApiResult<Option<Block>> {
        Ok(self.eth.block_by_hash(hash)?)
    }

    /// Try to construct a block from given payload. Perform addition validation of `extra_data` and
    /// `base_fee_per_gas` fields.
    ///
    /// NOTE: The log bloom is assumed to be validated during serialization.
    /// NOTE: Ommers hash is validated upon computing block hash and comparing the value with
    /// `payload.block_hash`.
    /// Ref: https://github.com/ethereum/go-ethereum/blob/79a478bb6176425c2400e949890e668a3d9a3d05/core/beacon/types.go#L145
    fn try_construct_block(&self, payload: ExecutionPayload) -> EngineApiResult<BlockLocked> {
        if payload.extra_data.len() > 32 {
            return Err(EngineApiError::PayloadExtraData(payload.extra_data))
        }

        if payload.base_fee_per_gas.is_zero() {
            return Err(EngineApiError::PayloadBaseFee(payload.base_fee_per_gas))
        }

        let transactions = payload
            .transactions
            .iter()
            .map(|tx| TransactionSigned::decode(&mut tx.as_ref()))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let transactions_root = proofs::calculate_transaction_root(transactions.iter());
        let header = Header {
            parent_hash: payload.parent_hash,
            beneficiary: payload.fee_recipient,
            state_root: payload.state_root,
            transactions_root,
            receipts_root: payload.receipts_root,
            logs_bloom: payload.logs_bloom,
            number: payload.block_number.as_u64(),
            gas_limit: payload.gas_limit.as_u64(),
            gas_used: payload.gas_used.as_u64(),
            timestamp: payload.timestamp.as_u64(),
            mix_hash: payload.prev_randao,
            base_fee_per_gas: Some(payload.base_fee_per_gas.as_u64()),
            extra_data: payload.extra_data.0,
            // Defaults
            ommers_hash: EMPTY_LIST_HASH,
            difficulty: Default::default(),
            nonce: Default::default(),
        };
        let header = header.seal();

        if payload.block_hash != header.hash() {
            return Err(EngineApiError::PayloadBlockHash {
                execution: header.hash(),
                consensus: payload.block_hash,
            })
        }

        Ok(BlockLocked { header, body: transactions, ommers: Default::default() })
    }
}

#[async_trait]
impl<Eth> EngineApiServer for EngineApi<Eth>
where
    Eth: EthApiSpec + 'static,
{
    /// See also <https://github.com/ethereum/execution-apis/blob/8db51dcd2f4bdfbd9ad6e4a7560aac97010ad063/src/engine/specification.md#engine_newpayloadv1>
    /// Caution: This should not accept the `withdrawals` field
    async fn new_payload_v1(&self, payload: ExecutionPayload) -> Result<PayloadStatus> {
        let block: BlockLocked = match self.try_construct_block(payload) {
            Ok(b) => b,
            Err(err) => {
                return Ok(PayloadStatus::from_status(PayloadStatusEnum::InvalidBlockHash {
                    validation_error: err.to_string(),
                }))
            }
        };

        // The block already exists in our database
        if let Some(existing) = self.block_by_hash(block.hash())? {
            return Ok(PayloadStatus::new(PayloadStatusEnum::Valid, block.hash()))
        }

        let Some(parent) = self.block_by_hash(block.parent_hash)? else {
             // TODO: cache block for storing later
             return Ok(PayloadStatus::from_status(PayloadStatusEnum::Syncing))
        };

        if parent.number + 1 != block.number {
            // Our database is incongruent, potential reorg
            // TODO: handle
            todo!()
        }

        // TODO: sanity checks on td & timestamp
        // https://github.com/ethereum/go-ethereum/blob/79a478bb6176425c2400e949890e668a3d9a3d05/eth/catalyst/api.go#L403-L414

        return Ok(PayloadStatus::new(PayloadStatusEnum::Valid, block.hash()))
    }

    /// See also <https://github.com/ethereum/execution-apis/blob/8db51dcd2f4bdfbd9ad6e4a7560aac97010ad063/src/engine/specification.md#engine_newpayloadv1>
    async fn new_payload_v2(&self, _payload: ExecutionPayload) -> Result<PayloadStatus> {
        todo!()
    }

    /// See also <https://github.com/ethereum/execution-apis/blob/8db51dcd2f4bdfbd9ad6e4a7560aac97010ad063/src/engine/specification.md#engine_forkchoiceUpdatedV1>
    ///
    /// Caution: This should not accept the `withdrawals` field
    async fn fork_choice_updated_v1(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<PayloadAttributes>,
    ) -> Result<ForkchoiceUpdated> {
        let ForkchoiceState { head_block_hash, finalized_block_hash, .. } = fork_choice_state;

        if head_block_hash.is_zero() {
            return Ok(ForkchoiceUpdated::from_status(PayloadStatusEnum::Invalid {
                validation_error: "Empty head".to_owned(),
            }))
        }

        // The finalized block hash is not known, we are still syncing
        if !finalized_block_hash.is_zero() && self.block_by_hash(finalized_block_hash)?.is_none() {
            return Ok(ForkchoiceUpdated::from_status(PayloadStatusEnum::Syncing))
        }

        // TODO: record head

        let chain_info = self.eth.chain_info().with_message("Failed to read chain info")?;
        Ok(ForkchoiceUpdated::from_status(PayloadStatusEnum::Valid)
            .with_latest_valid_hash(chain_info.best_hash))
    }

    /// See also <https://github.com/ethereum/execution-apis/blob/main/src/engine/specification.md#engine_forkchoiceupdatedv2>
    async fn fork_choice_updated_v2(
        &self,
        _fork_choice_state: ForkchoiceState,
        _payload_attributes: Option<PayloadAttributes>,
    ) -> Result<ForkchoiceUpdated> {
        todo!()
    }

    /// See also <https://github.com/ethereum/execution-apis/blob/8db51dcd2f4bdfbd9ad6e4a7560aac97010ad063/src/engine/specification.md#engine_getPayloadV1>
    ///
    /// Caution: This should not return the `withdrawals` field
    async fn get_payload_v1(&self, _payload_id: H64) -> Result<ExecutionPayload> {
        // NOTE: Currently, we are not a builder
        Err(EngineApiError::PayloadUnknown.into())
    }

    /// See also <https://github.com/ethereum/execution-apis/blob/main/src/engine/specification.md#engine_getpayloadv2>
    async fn get_payload_v2(&self, _payload_id: H64) -> Result<ExecutionPayload> {
        // NOTE: Currently, we are not a builder
        Err(EngineApiError::PayloadUnknown.into())
    }

    /// See also <https://github.com/ethereum/execution-apis/blob/8db51dcd2f4bdfbd9ad6e4a7560aac97010ad063/src/engine/specification.md#engine_exchangeTransitionConfigurationV1>
    async fn exchange_transition_configuration(
        &self,
        transition_configuration: TransitionConfiguration,
    ) -> Result<TransitionConfiguration> {
        let TransitionConfiguration {
            terminal_total_difficulty,
            terminal_block_hash,
            terminal_block_number,
        } = transition_configuration;

        // Compare total difficulty values
        let merge_terminal_td = self.config.merge_terminal_total_difficulty.into();
        if merge_terminal_td != terminal_total_difficulty {
            return Err(EngineApiError::TerminalTD {
                execution: merge_terminal_td,
                consensus: terminal_total_difficulty,
            }
            .into())
        }

        // Short circuit if communicated block hash is zero
        if terminal_block_hash.is_zero() {
            return Ok(TransitionConfiguration {
                terminal_total_difficulty: merge_terminal_td,
                ..Default::default()
            })
        }

        // Attempt to look up terminal block hash
        let local_hash = self
            .eth
            .block_hash(terminal_block_number)
            .with_message("Failed to get block by hash")?;

        // Transition configuration exchange is successful if block hashes match
        match local_hash {
            Some(hash) if hash == terminal_block_hash => Ok(TransitionConfiguration {
                terminal_total_difficulty: merge_terminal_td,
                terminal_block_hash,
                terminal_block_number,
            }),
            _ => Err(EngineApiError::TerminalBlockHash {
                execution: local_hash,
                consensus: terminal_block_hash,
            }
            .into()),
        }
    }
}
