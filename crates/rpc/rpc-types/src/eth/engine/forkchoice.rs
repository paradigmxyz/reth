use super::{PayloadStatus, PayloadStatusEnum};
use crate::engine::PayloadId;
use reth_primitives::H256;
use serde::{Deserialize, Serialize};

/// This structure encapsulates the fork choice state
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForkchoiceState {
    pub head_block_hash: H256,
    pub safe_block_hash: H256,
    pub finalized_block_hash: H256,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForkchoiceUpdated {
    pub payload_status: PayloadStatus,
    pub payload_id: Option<PayloadId>,
}

impl ForkchoiceUpdated {
    pub fn new(payload_status: PayloadStatus) -> Self {
        Self { payload_status, payload_id: None }
    }

    pub fn from_status(status: PayloadStatusEnum) -> Self {
        Self { payload_status: PayloadStatus::from_status(status), payload_id: None }
    }

    pub fn with_latest_valid_hash(mut self, hash: H256) -> Self {
        self.payload_status.latest_valid_hash = Some(hash);
        self
    }

    pub fn with_payload_id(mut self, id: PayloadId) -> Self {
        self.payload_id = Some(id);
        self
    }
}
