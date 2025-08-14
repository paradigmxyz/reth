use jsonrpsee::proc_macros::rpc;
use jsonrpsee_core::{server::RpcModule, RpcResult};
use alloy_primitives::B256;

#[cfg(feature = "std")]
use once_cell::sync::OnceCell;
#[cfg(feature = "std")]
use std::sync::Arc;

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

#[cfg(feature = "std")]
#[async_trait::async_trait]
pub trait ArbNitroBackend: Send + Sync {
    async fn new_message(
        &self,
        msg_idx: u64,
        msg: serde_json::Value,
        msg_for_prefetch: Option<serde_json::Value>,
    ) -> Result<ArbMessageResult, String>;

    async fn reorg(
        &self,
        first_msg_to_add: u64,
        new_messages: Vec<serde_json::Value>,
        old_messages: Vec<serde_json::Value>,
    ) -> Result<Vec<ArbMessageResult>, String>;

    async fn head_message_index(&self) -> Result<u64, String>;

    async fn result_at_message_index(&self, msg_idx: u64) -> Result<ArbMessageResult, String>;

    async fn message_index_to_block_number(&self, msg_idx: u64) -> Result<u64, String>;

    async fn block_number_to_message_index(&self, block_number: u64) -> Result<u64, String>;

    async fn set_finality_data(
        &self,
        safe: Option<serde_json::Value>,
        finalized: Option<serde_json::Value>,
        validated: Option<serde_json::Value>,
    ) -> Result<(), String>;

    async fn mark_feed_start(&self, to: u64) -> Result<(), String>;

    async fn trigger_maintenance(&self) -> Result<(), String>;

    async fn should_trigger_maintenance(&self) -> Result<bool, String>;

    async fn maintenance_status(&self) -> Result<ArbMaintenanceStatus, String>;

    async fn record_block_creation(
        &self,
        pos: u64,
        msg: serde_json::Value,
    ) -> Result<ArbRecordResult, String>;

    async fn mark_valid(&self, pos: u64, result_hash: B256) -> Result<(), String>;

    async fn prepare_for_record(&self, start: u64, end: u64) -> Result<(), String>;

    async fn pause_sequencer(&self) -> Result<(), String>;

    async fn activate_sequencer(&self) -> Result<(), String>;

    async fn forward_to(&self, url: String) -> Result<(), String>;

    async fn sequence_delayed_message(
        &self,
        message: serde_json::Value,
        delayed_seq_num: u64,
    ) -> Result<(), String>;

    async fn next_delayed_message_number(&self) -> Result<u64, String>;

    async fn synced(&self) -> Result<bool, String>;

    async fn full_sync_progress(&self) -> Result<serde_json::Value, String>;

    async fn arbos_version_for_message_index(&self, msg_idx: u64) -> Result<u64, String>;
}

#[cfg(feature = "std")]
static ARB_BACKEND: OnceCell<Arc<dyn ArbNitroBackend>> = OnceCell::new();

#[cfg(feature = "std")]
pub fn set_arb_nitro_backend(backend: Arc<dyn ArbNitroBackend>) -> bool {
    ARB_BACKEND.set(backend).is_ok()
}

#[async_trait::async_trait]
impl ArbNitroApiServer for ArbNitroRpc {
    async fn new_message(
        &self,
        msg_idx: u64,
        msg: serde_json::Value,
        msg_for_prefetch: Option<serde_json::Value>,
    ) -> RpcResult<ArbMessageResult> {
        #[cfg(feature = "std")]
        if let Some(b) = ARB_BACKEND.get() {
            return b
                .new_message(msg_idx, msg, msg_for_prefetch)
                .await
                .map_err(|e| jsonrpsee_types::ErrorObjectOwned::owned(-32000, e, None::<()>).into());
        }
        Ok(ArbMessageResult { block_hash: B256::ZERO, send_root: B256::ZERO })
    }

    async fn reorg(
        &self,
        first_msg_to_add: u64,
        new_messages: Vec<serde_json::Value>,
        old_messages: Vec<serde_json::Value>,
    ) -> RpcResult<Vec<ArbMessageResult>> {
        #[cfg(feature = "std")]
        if let Some(b) = ARB_BACKEND.get() {
            return b
                .reorg(first_msg_to_add, new_messages, old_messages)
                .await
                .map_err(|e| jsonrpsee_types::ErrorObjectOwned::owned(-32000, e, None::<()>).into());
        }
        Ok(vec![])
    }

    async fn head_message_index(&self) -> RpcResult<u64> {
        #[cfg(feature = "std")]
        if let Some(b) = ARB_BACKEND.get() {
            return b.head_message_index().await.map_err(|e| jsonrpsee_types::ErrorObjectOwned::owned(-32000, e, None::<()>).into());
        }
        Ok(0)
    }

    async fn result_at_message_index(&self, msg_idx: u64) -> RpcResult<ArbMessageResult> {
        #[cfg(feature = "std")]
        if let Some(b) = ARB_BACKEND.get() {
            return b
                .result_at_message_index(msg_idx)
                .await
                .map_err(|e| jsonrpsee_types::ErrorObjectOwned::owned(-32000, e, None::<()>).into());
        }
        Ok(ArbMessageResult { block_hash: B256::ZERO, send_root: B256::ZERO })
    }

    async fn message_index_to_block_number(&self, msg_idx: u64) -> RpcResult<u64> {
        #[cfg(feature = "std")]
        if let Some(b) = ARB_BACKEND.get() {
            return b
                .message_index_to_block_number(msg_idx)
                .await
                .map_err(|e| jsonrpsee_types::ErrorObjectOwned::owned(-32000, e, None::<()>).into());
        }
        Ok(msg_idx)
    }

    async fn block_number_to_message_index(&self, block_number: u64) -> RpcResult<u64> {
        #[cfg(feature = "std")]
        if let Some(b) = ARB_BACKEND.get() {
            return b
                .block_number_to_message_index(block_number)
                .await
                .map_err(|e| jsonrpsee_types::ErrorObjectOwned::owned(-32000, e, None::<()>).into());
        }
        Ok(block_number)
    }

    async fn set_finality_data(
        &self,
        safe: Option<serde_json::Value>,
        finalized: Option<serde_json::Value>,
        validated: Option<serde_json::Value>,
    ) -> RpcResult<()> {
        #[cfg(feature = "std")]
        if let Some(b) = ARB_BACKEND.get() {
            return b
                .set_finality_data(safe, finalized, validated)
                .await
                .map_err(|e| jsonrpsee_types::ErrorObjectOwned::owned(-32000, e, None::<()>).into());
        }
        Ok(())
    }

    async fn mark_feed_start(&self, to: u64) -> RpcResult<()> {
        #[cfg(feature = "std")]
        if let Some(b) = ARB_BACKEND.get() {
            return b.mark_feed_start(to).await.map_err(|e| jsonrpsee_types::ErrorObjectOwned::owned(-32000, e, None::<()>).into());
        }
        Ok(())
    }

    async fn trigger_maintenance(&self) -> RpcResult<()> {
        #[cfg(feature = "std")]
        if let Some(b) = ARB_BACKEND.get() {
            return b.trigger_maintenance().await.map_err(|e| jsonrpsee_types::ErrorObjectOwned::owned(-32000, e, None::<()>).into());
        }
        Ok(())
    }

    async fn should_trigger_maintenance(&self) -> RpcResult<bool> {
        #[cfg(feature = "std")]
        if let Some(b) = ARB_BACKEND.get() {
            return b
                .should_trigger_maintenance()
                .await
                .map_err(|e| jsonrpsee_types::ErrorObjectOwned::owned(-32000, e, None::<()>).into());
        }
        Ok(false)
    }

    async fn maintenance_status(&self) -> RpcResult<ArbMaintenanceStatus> {
        #[cfg(feature = "std")]
        if let Some(b) = ARB_BACKEND.get() {
            return b.maintenance_status().await.map_err(|e| jsonrpsee_types::ErrorObjectOwned::owned(-32000, e, None::<()>).into());
        }
        Ok(ArbMaintenanceStatus { status: "ok".to_string() })
    }

    async fn record_block_creation(
        &self,
        pos: u64,
        msg: serde_json::Value,
    ) -> RpcResult<ArbRecordResult> {
        #[cfg(feature = "std")]
        if let Some(b) = ARB_BACKEND.get() {
            return b
                .record_block_creation(pos, msg)
                .await
                .map_err(|e| jsonrpsee_types::ErrorObjectOwned::owned(-32000, e, None::<()>).into());
        }
        Ok(ArbRecordResult { result_hash: B256::ZERO })
    }

    async fn mark_valid(&self, pos: u64, result_hash: B256) -> RpcResult<()> {
        #[cfg(feature = "std")]
        if let Some(b) = ARB_BACKEND.get() {
            return b.mark_valid(pos, result_hash).await.map_err(|e| jsonrpsee_types::ErrorObjectOwned::owned(-32000, e, None::<()>).into());
        }
        Ok(())
    }

    async fn prepare_for_record(&self, start: u64, end: u64) -> RpcResult<()> {
        #[cfg(feature = "std")]
        if let Some(b) = ARB_BACKEND.get() {
            return b
                .prepare_for_record(start, end)
                .await
                .map_err(|e| jsonrpsee_types::ErrorObjectOwned::owned(-32000, e, None::<()>).into());
        }
        Ok(())
    }

    async fn pause_sequencer(&self) -> RpcResult<()> {
        #[cfg(feature = "std")]
        if let Some(b) = ARB_BACKEND.get() {
            return b.pause_sequencer().await.map_err(|e| jsonrpsee_types::ErrorObjectOwned::owned(-32000, e, None::<()>).into());
        }
        Ok(())
    }

    async fn activate_sequencer(&self) -> RpcResult<()> {
        #[cfg(feature = "std")]
        if let Some(b) = ARB_BACKEND.get() {
            return b.activate_sequencer().await.map_err(|e| jsonrpsee_types::ErrorObjectOwned::owned(-32000, e, None::<()>).into());
        }
        Ok(())
    }

    async fn forward_to(&self, url: String) -> RpcResult<()> {
        #[cfg(feature = "std")]
        if let Some(b) = ARB_BACKEND.get() {
            return b.forward_to(url).await.map_err(|e| jsonrpsee_types::ErrorObjectOwned::owned(-32000, e, None::<()>).into());
        }
        Ok(())
    }

    async fn sequence_delayed_message(
        &self,
        message: serde_json::Value,
        delayed_seq_num: u64,
    ) -> RpcResult<()> {
        #[cfg(feature = "std")]
        if let Some(b) = ARB_BACKEND.get() {
            return b
                .sequence_delayed_message(message, delayed_seq_num)
                .await
                .map_err(|e| jsonrpsee_types::ErrorObjectOwned::owned(-32000, e, None::<()>).into());
        }
        Ok(())
    }

    async fn next_delayed_message_number(&self) -> RpcResult<u64> {
        #[cfg(feature = "std")]
        if let Some(b) = ARB_BACKEND.get() {
            return b
                .next_delayed_message_number()
                .await
                .map_err(|e| jsonrpsee_types::ErrorObjectOwned::owned(-32000, e, None::<()>).into());
        }
        Ok(0)
    }

    async fn synced(&self) -> RpcResult<bool> {
        #[cfg(feature = "std")]
        if let Some(b) = ARB_BACKEND.get() {
            return b.synced().await.map_err(|e| jsonrpsee_types::ErrorObjectOwned::owned(-32000, e, None::<()>).into());
        }
        Ok(true)
    }

    async fn full_sync_progress(&self) -> RpcResult<serde_json::Value> {
        #[cfg(feature = "std")]
        if let Some(b) = ARB_BACKEND.get() {
            return b
                .full_sync_progress()
                .await
                .map_err(|e| jsonrpsee_types::ErrorObjectOwned::owned(-32000, e, None::<()>).into());
        }
        Ok(serde_json::json!({"status": "idle"}))
    }

    async fn arbos_version_for_message_index(&self, msg_idx: u64) -> RpcResult<u64> {
        #[cfg(feature = "std")]
        if let Some(b) = ARB_BACKEND.get() {
            return b
                .arbos_version_for_message_index(msg_idx)
                .await
                .map_err(|e| jsonrpsee_types::ErrorObjectOwned::owned(-32000, e, None::<()>).into());
        }
        Ok(1)
    }
}
