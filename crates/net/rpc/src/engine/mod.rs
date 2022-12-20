use crate::{
    result::{rpc_err, ToRpcResult},
    EthApiSpec,
};
use async_trait::async_trait;
use jsonrpsee::core::RpcResult as Result;
use reth_consensus::Config;
use reth_interfaces::consensus::ForkchoiceState;
use reth_primitives::{H256, H64};
use reth_rpc_api::EngineApiServer;
use reth_rpc_types::engine::{
    ExecutionPayload, ForkchoiceUpdated, PayloadAttributes, PayloadStatus, PayloadStatusEnum,
    TransitionConfiguration,
};

mod error;
pub use error::EngineApiError;

/// TODO:
pub struct EngineApi {
    /// The implementation of `eth` API
    eth: Box<dyn EthApiSpec>,
    /// Consensus configuration
    config: Config,
}

impl std::fmt::Debug for EngineApi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineApi").finish_non_exhaustive()
    }
}

impl EngineApi {
    fn get_local_transition_configuration(&self) -> TransitionConfiguration {
        return TransitionConfiguration {
            terminal_total_difficulty: self.config.merge_terminal_total_difficulty.into(),
            terminal_block_hash: H256::zero(),
            terminal_block_number: 0,
        }
    }
}

#[async_trait]
impl EngineApiServer for EngineApi {
    /// See also <https://github.com/ethereum/execution-apis/blob/8db51dcd2f4bdfbd9ad6e4a7560aac97010ad063/src/engine/specification.md#engine_newpayloadv1>
    /// Caution: This should not accept the `withdrawals` field
    async fn new_payload_v1(&self, _payload: ExecutionPayload) -> Result<PayloadStatus> {
        todo!()
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
            return Ok(ForkchoiceUpdated::new(PayloadStatusEnum::InvalidBlockHash {
                validation_error: "Empty head".to_owned(),
            }))
        }

        if !finalized_block_hash.is_zero() &&
            self.eth
                .block_by_hash(finalized_block_hash)
                .with_message("failed to get block by hash")? // TODO: extract
                .is_none()
        {
            return Ok(ForkchoiceUpdated::new(PayloadStatusEnum::Syncing))
        }

        // TODO: record head

        let chain_info = self.eth.chain_info().with_message("failed to read chain info")?;
        Ok(ForkchoiceUpdated::new(PayloadStatusEnum::Valid)
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
        // NOTE: Currently we are not a builder
        Err(EngineApiError::UnknownPayload.into())
    }

    /// See also <https://github.com/ethereum/execution-apis/blob/main/src/engine/specification.md#engine_getpayloadv2>
    async fn get_payload_v2(&self, _payload_id: H64) -> Result<ExecutionPayload> {
        // NOTE: Currently we are not a builder
        Err(EngineApiError::UnknownPayload.into())
    }

    /// See also <https://github.com/ethereum/execution-apis/blob/8db51dcd2f4bdfbd9ad6e4a7560aac97010ad063/src/engine/specification.md#engine_exchangeTransitionConfigurationV1>
    async fn exchange_transition_configuration(
        &self,
        transition_configuration: TransitionConfiguration,
    ) -> Result<TransitionConfiguration> {
        let local = self.get_local_transition_configuration();
        if local.terminal_total_difficulty != transition_configuration.terminal_total_difficulty {
            return Err(EngineApiError::TerminalTD {
                expected: local.terminal_total_difficulty,
                received: transition_configuration.terminal_total_difficulty,
            }
            .into())
        }

        if local.terminal_block_hash != transition_configuration.terminal_block_hash {
            return Err(EngineApiError::TerminalBlockHash {
                expected: local.terminal_block_hash,
                received: transition_configuration.terminal_block_hash,
            }
            .into())
        }

        if local.terminal_block_number != transition_configuration.terminal_block_number {
            return Err(EngineApiError::TerminalBlockNumber {
                expected: local.terminal_block_number,
                received: transition_configuration.terminal_block_number,
            }
            .into())
        }

        Ok(local)
    }
}
