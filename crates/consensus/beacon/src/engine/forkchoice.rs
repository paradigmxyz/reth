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
    /// If the status is `VALID`, we also update the last valid forkchoice state and set the
    /// `sync_target` to `None`, since we're now fully synced.
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

    /// Returns the [ForkchoiceStatus] of the latest received FCU.
    ///
    /// Caution: this can be invalid.
    pub(crate) fn latest_status(&self) -> Option<ForkchoiceStatus> {
        self.latest.as_ref().map(|s| s.status)
    }

    /// Returns whether the latest received FCU is valid: [ForkchoiceStatus::Valid]
    #[allow(unused)]
    pub(crate) fn is_latest_valid(&self) -> bool {
        self.latest_status().map(|s| s.is_valid()).unwrap_or(false)
    }

    /// Returns whether the latest received FCU is syncing: [ForkchoiceStatus::Syncing]
    #[allow(unused)]
    pub(crate) fn is_latest_syncing(&self) -> bool {
        self.latest_status().map(|s| s.is_syncing()).unwrap_or(false)
    }

    /// Returns whether the latest received FCU is syncing: [ForkchoiceStatus::Invalid]
    #[allow(unused)]
    pub(crate) fn is_latest_invalid(&self) -> bool {
        self.latest_status().map(|s| s.is_invalid()).unwrap_or(false)
    }

    /// Returns the last valid head hash.
    #[allow(unused)]
    pub(crate) fn last_valid_head(&self) -> Option<H256> {
        self.last_valid.as_ref().map(|s| s.head_block_hash)
    }

    /// Returns the head hash of the latest received FCU to which we need to sync.
    #[allow(unused)]
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
pub enum ForkchoiceStatus {
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

    pub(crate) fn is_invalid(&self) -> bool {
        matches!(self, ForkchoiceStatus::Invalid)
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
        }
    }
}

impl From<PayloadStatusEnum> for ForkchoiceStatus {
    fn from(status: PayloadStatusEnum) -> Self {
        ForkchoiceStatus::from_payload_status(&status)
    }
}

/// A helper type to check represent hashes of a [ForkchoiceState]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ForkchoiceStateHash {
    Head(H256),
    Safe(H256),
    Finalized(H256),
}

impl ForkchoiceStateHash {
    /// Tries to find a matching hash in the given [ForkchoiceState].
    pub(crate) fn find(state: &ForkchoiceState, hash: H256) -> Option<Self> {
        if state.head_block_hash == hash {
            Some(ForkchoiceStateHash::Head(hash))
        } else if state.safe_block_hash == hash {
            Some(ForkchoiceStateHash::Safe(hash))
        } else if state.finalized_block_hash == hash {
            Some(ForkchoiceStateHash::Finalized(hash))
        } else {
            None
        }
    }

    /// Returns true if this is the head hash of the [ForkchoiceState]
    pub(crate) fn is_head(&self) -> bool {
        matches!(self, ForkchoiceStateHash::Head(_))
    }
}

impl AsRef<H256> for ForkchoiceStateHash {
    fn as_ref(&self) -> &H256 {
        match self {
            ForkchoiceStateHash::Head(h) => h,
            ForkchoiceStateHash::Safe(h) => h,
            ForkchoiceStateHash::Finalized(h) => h,
        }
    }
}
