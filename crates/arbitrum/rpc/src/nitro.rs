use jsonrpsee::proc_macros::rpc;
use jsonrpsee_core::{server::RpcModule, RpcResult};
use alloy_primitives::B256;

#[derive(Debug, Clone, Default)]
pub struct ArbNitroRpc;

#[rpc(server, namespace = "arb")]
pub trait ArbNitroApi {
    #[method(name = "newMessage")]
    async fn new_message(
        &self,
        msg_idx: u64,
        msg: serde_json::Value,
        msg_for_prefetch: Option<serde_json::Value>,
    ) -> RpcResult<ArbMessageResult>;

    #[method(name = "reorg")]
    async fn reorg(
        &self,
        first_msg_to_add: u64,
        new_messages: Vec<serde_json::Value>,
        old_messages: Vec<serde_json::Value>,
    ) -> RpcResult<Vec<ArbMessageResult>>;

    #[method(name = "headMessageIndex")]
    async fn head_message_index(&self) -> RpcResult<u64>;

    #[method(name = "resultAtMessageIndex")]
    async fn result_at_message_index(&self, msg_idx: u64) -> RpcResult<ArbMessageResult>;

    #[method(name = "messageIndexToBlockNumber")]
    async fn message_index_to_block_number(&self, msg_idx: u64) -> RpcResult<u64>;

    #[method(name = "blockNumberToMessageIndex")]
    async fn block_number_to_message_index(&self, block_number: u64) -> RpcResult<u64>;

    #[method(name = "setFinalityData")]
    async fn set_finality_data(
        &self,
        safe: Option<serde_json::Value>,
        finalized: Option<serde_json::Value>,
        validated: Option<serde_json::Value>,
    ) -> RpcResult<()>;

    #[method(name = "markFeedStart")]
    async fn mark_feed_start(&self, to: u64) -> RpcResult<()>;

    #[method(name = "triggerMaintenance")]
    async fn trigger_maintenance(&self) -> RpcResult<()>;

    #[method(name = "shouldTriggerMaintenance")]
    async fn should_trigger_maintenance(&self) -> RpcResult<bool>;

    #[method(name = "maintenanceStatus")]
    async fn maintenance_status(&self) -> RpcResult<ArbMaintenanceStatus>;

    #[method(name = "recordBlockCreation")]
    async fn record_block_creation(
        &self,
        pos: u64,
        msg: serde_json::Value,
    ) -> RpcResult<ArbRecordResult>;

    #[method(name = "markValid")]
    async fn mark_valid(&self, pos: u64, result_hash: B256) -> RpcResult<()>;

    #[method(name = "prepareForRecord")]
    async fn prepare_for_record(&self, start: u64, end: u64) -> RpcResult<()>;

    #[method(name = "pauseSequencer")]
    async fn pause_sequencer(&self) -> RpcResult<()>;

    #[method(name = "activateSequencer")]
    async fn activate_sequencer(&self) -> RpcResult<()>;

    #[method(name = "forwardTo")]
    async fn forward_to(&self, url: String) -> RpcResult<()>;

    #[method(name = "sequenceDelayedMessage")]
    async fn sequence_delayed_message(
        &self,
        message: serde_json::Value,
        delayed_seq_num: u64,
    ) -> RpcResult<()>;

    #[method(name = "nextDelayedMessageNumber")]
    async fn next_delayed_message_number(&self) -> RpcResult<u64>;

    #[method(name = "synced")]
    async fn synced(&self) -> RpcResult<bool>;

    #[method(name = "fullSyncProgress")]
    async fn full_sync_progress(&self) -> RpcResult<serde_json::Value>;

    #[method(name = "arbosVersionForMessageIndex")]
    async fn arbos_version_for_message_index(&self, msg_idx: u64) -> RpcResult<u64>;
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ArbMessageResult {
    pub block_hash: B256,
    pub send_root: B256,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ArbMaintenanceStatus {
    pub status: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ArbRecordResult {
    pub result_hash: B256,
}

impl ArbNitroRpc {
    pub fn into_rpc_module(self) -> RpcModule<()> {
        self.into_rpc().remove_context()
    }
}

#[async_trait::async_trait]
impl ArbNitroApiServer for ArbNitroRpc {
    async fn new_message(
        &self,
        _msg_idx: u64,
        _msg: serde_json::Value,
        _msg_for_prefetch: Option<serde_json::Value>,
    ) -> RpcResult<ArbMessageResult> {
        Ok(ArbMessageResult { block_hash: B256::ZERO, send_root: B256::ZERO })
    }

    async fn reorg(
        &self,
        _first_msg_to_add: u64,
        _new_messages: Vec<serde_json::Value>,
        _old_messages: Vec<serde_json::Value>,
    ) -> RpcResult<Vec<ArbMessageResult>> {
        Ok(vec![])
    }

    async fn head_message_index(&self) -> RpcResult<u64> {
        Ok(0)
    }

    async fn result_at_message_index(&self, _msg_idx: u64) -> RpcResult<ArbMessageResult> {
        Ok(ArbMessageResult { block_hash: B256::ZERO, send_root: B256::ZERO })
    }

    async fn message_index_to_block_number(&self, msg_idx: u64) -> RpcResult<u64> {
        Ok(msg_idx)
    }

    async fn block_number_to_message_index(&self, block_number: u64) -> RpcResult<u64> {
        Ok(block_number)
    }

    async fn set_finality_data(
        &self,
        _safe: Option<serde_json::Value>,
        _finalized: Option<serde_json::Value>,
        _validated: Option<serde_json::Value>,
    ) -> RpcResult<()> {
        Ok(())
    }

    async fn mark_feed_start(&self, _to: u64) -> RpcResult<()> {
        Ok(())
    }

    async fn trigger_maintenance(&self) -> RpcResult<()> {
        Ok(())
    }

    async fn should_trigger_maintenance(&self) -> RpcResult<bool> {
        Ok(false)
    }

    async fn maintenance_status(&self) -> RpcResult<ArbMaintenanceStatus> {
        Ok(ArbMaintenanceStatus { status: "ok".to_string() })
    }

    async fn record_block_creation(
        &self,
        _pos: u64,
        _msg: serde_json::Value,
    ) -> RpcResult<ArbRecordResult> {
        Ok(ArbRecordResult { result_hash: B256::ZERO })
    }

    async fn mark_valid(&self, _pos: u64, _result_hash: B256) -> RpcResult<()> {
        Ok(())
    }

    async fn prepare_for_record(&self, _start: u64, _end: u64) -> RpcResult<()> {
        Ok(())
    }

    async fn pause_sequencer(&self) -> RpcResult<()> {
        Ok(())
    }

    async fn activate_sequencer(&self) -> RpcResult<()> {
        Ok(())
    }

    async fn forward_to(&self, _url: String) -> RpcResult<()> {
        Ok(())
    }

    async fn sequence_delayed_message(
        &self,
        _message: serde_json::Value,
        _delayed_seq_num: u64,
    ) -> RpcResult<()> {
        Ok(())
    }

    async fn next_delayed_message_number(&self) -> RpcResult<u64> {
        Ok(0)
    }

    async fn synced(&self) -> RpcResult<bool> {
        Ok(true)
    }

    async fn full_sync_progress(&self) -> RpcResult<serde_json::Value> {
        Ok(serde_json::json!({"status": "idle"}))
    }

    async fn arbos_version_for_message_index(&self, _msg_idx: u64) -> RpcResult<u64> {
        Ok(1)
    }
}
