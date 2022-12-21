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

#[async_trait]
impl EngineApiServer for EngineApi {
    /// See also <https://github.com/ethereum/execution-apis/blob/8db51dcd2f4bdfbd9ad6e4a7560aac97010ad063/src/engine/specification.md#engine_newpayloadv1>
    /// Caution: This should not accept the `withdrawals` field
    async fn new_payload_v1(&self, _payload: ExecutionPayload) -> Result<PayloadStatus> {
        // TODO: execute payload first

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
            return Ok(ForkchoiceUpdated::new(PayloadStatusEnum::Invalid {
                validation_error: "Empty head".to_owned(),
            }))
        }

        if !finalized_block_hash.is_zero() &&
            self.eth
                .block_by_hash(finalized_block_hash)
                .with_message("Failed to get block by hash")? // TODO: extract
                .is_none()
        {
            return Ok(ForkchoiceUpdated::new(PayloadStatusEnum::Syncing))
        }

        // TODO: record head

        let chain_info = self.eth.chain_info().with_message("Failed to read chain info")?;
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
        // NOTE: Currently, we are not a builder
        Err(EngineApiError::UnknownPayload.into())
    }

    /// See also <https://github.com/ethereum/execution-apis/blob/main/src/engine/specification.md#engine_getpayloadv2>
    async fn get_payload_v2(&self, _payload_id: H64) -> Result<ExecutionPayload> {
        // NOTE: Currently, we are not a builder
        Err(EngineApiError::UnknownPayload.into())
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
