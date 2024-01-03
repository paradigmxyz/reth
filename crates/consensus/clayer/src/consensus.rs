use alloy_primitives::B256;
use parking_lot::RwLock;
use reth_interfaces::clayer::ClayerConsensus;
use reth_rpc_types::engine::{
    ExecutionPayloadEnvelopeV2, ExecutionPayloadFieldV2, ExecutionPayloadInputV2, ForkchoiceState,
    ForkchoiceUpdated, PayloadAttributes, PayloadStatus,
};
use std::{collections::VecDeque, sync::Arc};
use tracing::error;

use tokio::sync::{
    mpsc,
    mpsc::{Receiver, Sender},
};

use crate::engine_api::{http::HttpJsonRpc, ClRpcError, ExecutionPayloadWrapperV2};

#[derive(Debug, Clone)]
pub struct ClayerConsensusEngine {
    pub inner: Arc<RwLock<ClayerConsensusEngineInner>>,
}

impl ClayerConsensusEngine {
    pub fn new(is_validator: bool) -> Self {
        Self { inner: Arc::new(RwLock::new(ClayerConsensusEngineInner::new(is_validator))) }
    }

    pub fn is_validator(&self) -> bool {
        self.inner.read().is_validator
    }

    pub async fn forkchoice_updated(
        api: &Arc<HttpJsonRpc>,
        last_block: B256,
    ) -> Result<ForkchoiceUpdated, ClRpcError> {
        let forkchoice_state = ForkchoiceState {
            head_block_hash: last_block,
            finalized_block_hash: last_block,
            safe_block_hash: last_block,
        };

        let response = api.forkchoice_updated_v2(forkchoice_state, None).await?;
        Ok(response)
    }

    pub async fn forkchoice_updated_with_attributes(
        api: &Arc<HttpJsonRpc>,
        last_block: B256,
    ) -> Result<ForkchoiceUpdated, ClRpcError> {
        let forkchoice_state = ForkchoiceState {
            head_block_hash: last_block,
            finalized_block_hash: last_block,
            safe_block_hash: last_block,
        };

        let data = r#"
            {
                "timestamp": "0x658967b8",
                "prevRandao": "0x0000000000000000000000000000000000000000000000000000000000000000",
                "suggestedFeeRecipient": "0x0000000000000000000000000000000000000000",
                "withdrawals": [
                    {
                        "index": "0x00",
                        "validatorIndex": "0x00",
                        "address": "0x00000000000000000000000000000000000010f0",
                        "amount": "0x1"
                    }
                ]
            }"#;
        let mut p: PayloadAttributes = serde_json::from_str(data).unwrap();
        let dt = chrono::prelude::Local::now();
        p.timestamp = dt.timestamp() as u64;
        let response = api.forkchoice_updated_v2(forkchoice_state, Some(p)).await?;
        Ok(response)
    }

    pub async fn new_payload(
        api: &Arc<HttpJsonRpc>,
        execution_payload: ExecutionPayloadWrapperV2,
    ) -> Result<PayloadStatus, ClRpcError> {
        let input = ExecutionPayloadInputV2 {
            execution_payload: execution_payload.execution_payload.payload_inner.clone(),
            withdrawals: Some(execution_payload.execution_payload.withdrawals.clone()),
        };
        let response = api.new_payload_v2(input).await?;

        Ok(response)
    }
}

impl ClayerConsensus for ClayerConsensusEngine {
    /// Returns pending consensus listener
    fn pending_consensus_listener(&self) -> Receiver<reth_primitives::Bytes> {
        self.inner.write().pending_consensus_listener()
    }

    /// push data received from network into cache
    fn push_cache(&self, data: reth_primitives::Bytes) {
        self.inner.write().push_cache(data);
    }
    /// pop data received from network out cache
    fn pop_cache(&self) -> Option<reth_primitives::Bytes> {
        self.inner.write().pop_cache()
    }
    /// broadcast consensus
    fn broadcast_consensus(&self, data: reth_primitives::Bytes) {
        self.inner.read().broadcast_consensus(data);
    }
}

#[derive(Debug, Clone)]
pub struct ClayerConsensusEngineInner {
    pub is_validator: bool,
    queued: VecDeque<reth_primitives::Bytes>,
    sender: Option<Sender<reth_primitives::Bytes>>,
}

impl ClayerConsensusEngineInner {
    pub fn new(is_validator: bool) -> Self {
        Self { is_validator, queued: VecDeque::default(), sender: None }
    }

    fn pending_consensus_listener(&mut self) -> Receiver<reth_primitives::Bytes> {
        let (sender, rx) = mpsc::channel(1024);
        self.sender = Some(sender);
        rx
    }

    fn push_cache(&mut self, data: reth_primitives::Bytes) {
        self.queued.push_back(data);
    }

    fn pop_cache(&mut self) -> Option<reth_primitives::Bytes> {
        self.queued.pop_front()
    }

    fn broadcast_consensus(&self, data: reth_primitives::Bytes) {
        if let Some(sender) = &self.sender {
            match sender.try_send(data) {
                Ok(()) => {}
                Err(err) => {
                    error!(target:"consensus::cl","broadcast_consensus error {:?}",err);
                }
            }
        }
    }
}
