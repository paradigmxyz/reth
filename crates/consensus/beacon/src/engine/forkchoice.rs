use reth_primitives::H256;
use reth_rpc_types::engine::{ForkchoiceState, PayloadStatusEnum};

/// The struct that keeps track of the received forkchoice state and their status.
#[derive(Debug, Clone, Default)]
pub(crate) struct ForkchoiceStateTracker {
    /// The latest forkchoice state that we received.
    ///
    /// Caution: this can be invalid.
    latest: Option<ReceivedForkchoiceState>,

    /// Tracks the latest forkchoice state that we received to which we need to sync.
    last_syncing: Option<ForkchoiceState>,
    /// The latest valid forkchoice state that we received and processed as valid.
    last_valid: Option<ForkchoiceState>,
}

impl ForkchoiceStateTracker {
    /// Sets the latest forkchoice state that we received.
    ///
    /// If the status is valid, we also update the last valid forkchoice state.
    pub(crate) fn set_latest(&mut self, state: ForkchoiceState, status: ForkchoiceStatus) {
        if status.is_valid() {
            self.set_valid(state);
        } else if status.is_syncing() {
            self.last_syncing = Some(state);
        }

        let received = ReceivedForkchoiceState { state, status };
        self.latest = Some(received);
    }

    fn set_valid(&mut self, state: ForkchoiceState) {
        // we no longer need to sync to this state.
        self.last_syncing = None;

        self.last_valid = Some(state);
    }

    /// Returns the head hash of the latest received FCU to which we need to sync.
    pub(crate) fn sync_target(&self) -> Option<H256> {
        self.last_syncing.as_ref().map(|s| s.head_block_hash)
    }

    /// Returns the last received ForkchoiceState to which we need to sync.
    pub(crate) fn sync_target_state(&self) -> Option<ForkchoiceState> {
        self.last_syncing
    }

    /// Returns true if no forkchoice state has been received yet.
    pub(crate) fn is_empty(&self) -> bool {
        self.latest.is_none()
    }
}

/// Represents a forkchoice update and tracks the status we assigned to it.
#[derive(Debug, Clone)]
#[allow(unused)]
pub(crate) struct ReceivedForkchoiceState {
    state: ForkchoiceState,
    status: ForkchoiceStatus,
}

/// A simplified representation of [PayloadStatusEnum] specifically for FCU.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum ForkchoiceStatus {
    /// The forkchoice state is valid.
    Valid,
    /// The forkchoice state is invalid.
    Invalid,
    /// The forkchoice state is unknown.
    Syncing,
}

impl ForkchoiceStatus {
    pub(crate) fn is_valid(&self) -> bool {
        matches!(self, ForkchoiceStatus::Valid)
    }

    pub(crate) fn is_syncing(&self) -> bool {
        matches!(self, ForkchoiceStatus::Syncing)
    }

    /// Converts the general purpose [PayloadStatusEnum] into a [ForkchoiceStatus].
    pub(crate) fn from_payload_status(status: &PayloadStatusEnum) -> Self {
        match status {
            PayloadStatusEnum::Valid => ForkchoiceStatus::Valid,
            PayloadStatusEnum::Invalid { .. } => ForkchoiceStatus::Invalid,
            PayloadStatusEnum::Syncing => ForkchoiceStatus::Syncing,
            PayloadStatusEnum::Accepted => {
                // This is only returned on `newPayload` accepted would be a valid state here.
                ForkchoiceStatus::Valid
            }
            PayloadStatusEnum::InvalidBlockHash { .. } => ForkchoiceStatus::Invalid,
        }
    }
}

impl From<PayloadStatusEnum> for ForkchoiceStatus {
    fn from(status: PayloadStatusEnum) -> Self {
        ForkchoiceStatus::from_payload_status(&status)
    }
}
