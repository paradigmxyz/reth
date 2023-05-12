use super::{PayloadStatus, PayloadStatusEnum};
use crate::engine::PayloadId;
use reth_primitives::H256;
use serde::{Deserialize, Serialize};

/// invalid forkchoice state error code.
pub const INVALID_FORK_CHOICE_STATE_ERROR: i32 = -38002;

/// invalid payload attributes error code.
pub const INVALID_PAYLOAD_ATTRIBUTES_ERROR: i32 = -38003;

/// invalid forkchoice state error message.
pub const INVALID_FORK_CHOICE_STATE_ERROR_MSG: &str = "Invalid forkchoice state";

/// invalid payload attributes error message.
pub const INVALID_PAYLOAD_ATTRIBUTES_ERROR_MSG: &str = "Invalid payload attributes";

/// Represents possible variants of a processed forkchoice update.
pub type ForkChoiceUpdateResult = Result<ForkchoiceUpdated, ForkchoiceUpdateError>;

/// This structure encapsulates the fork choice state
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForkchoiceState {
    /// Hash of the head block.
    pub head_block_hash: H256,
    /// Hash of the safe block.
    pub safe_block_hash: H256,
    /// Hash of finalized block.
    pub finalized_block_hash: H256,
}

/// A standalone forkchoice update result for RPC.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ForkchoiceUpdateError {
    /// The forkchoice update has been processed, but the requested contained invalid
    /// [PayloadAttributes](crate::engine::PayloadAttributes).
    ///
    /// This is returned as an error because the payload attributes are invalid and the payload is not valid, See <https://github.com/ethereum/execution-apis/blob/6709c2a795b707202e93c4f2867fa0bf2640a84f/src/engine/paris.md#engine_forkchoiceupdatedv1>
    #[error("Invalid payload attributes")]
    UpdatedInvalidPayloadAttributes,
    /// The given [ForkchoiceState] is invalid or inconsistent.
    #[error("Invalid forkchoice state")]
    InvalidState,
}

impl From<ForkchoiceUpdateError> for jsonrpsee_types::error::ErrorObject<'static> {
    fn from(value: ForkchoiceUpdateError) -> Self {
        match value {
            ForkchoiceUpdateError::UpdatedInvalidPayloadAttributes => {
                jsonrpsee_types::error::ErrorObject::owned(
                    INVALID_PAYLOAD_ATTRIBUTES_ERROR,
                    INVALID_PAYLOAD_ATTRIBUTES_ERROR_MSG,
                    None::<()>,
                )
            }
            ForkchoiceUpdateError::InvalidState => jsonrpsee_types::error::ErrorObject::owned(
                INVALID_FORK_CHOICE_STATE_ERROR,
                INVALID_FORK_CHOICE_STATE_ERROR_MSG,
                None::<()>,
            ),
        }
    }
}

/// Represents a successfully _processed_ forkchoice state update.
///
/// Note: this can still be INVALID if the provided payload was invalid.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForkchoiceUpdated {
    /// Represents the outcome of the validation of the payload, independently of the payload being
    /// valid or not.
    pub payload_status: PayloadStatus,
    /// The identifier of the payload build process that was successfully initiated.
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

    /// Returns true if the payload status is syncing.
    pub fn is_syncing(&self) -> bool {
        self.payload_status.is_syncing()
    }

    /// Returns true if the payload status is valid.
    pub fn is_valid(&self) -> bool {
        self.payload_status.is_valid()
    }

    /// Returns true if the payload status is invalid.
    pub fn is_invalid(&self) -> bool {
        self.payload_status.is_invalid()
    }
}
