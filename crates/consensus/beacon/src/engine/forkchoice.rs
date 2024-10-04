use alloy_primitives::B256;
use alloy_rpc_types_engine::{ForkchoiceState, PayloadStatusEnum};

/// The struct that keeps track of the received forkchoice state and their status.
#[derive(Debug, Clone, Default)]
pub struct ForkchoiceStateTracker {
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
    pub fn set_latest(&mut self, state: ForkchoiceState, status: ForkchoiceStatus) {
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

    /// Returns the [`ForkchoiceStatus`] of the latest received FCU.
    ///
    /// Caution: this can be invalid.
    pub(crate) fn latest_status(&self) -> Option<ForkchoiceStatus> {
        self.latest.as_ref().map(|s| s.status)
    }

    /// Returns whether the latest received FCU is valid: [`ForkchoiceStatus::Valid`]
    #[allow(dead_code)]
    pub(crate) fn is_latest_valid(&self) -> bool {
        self.latest_status().map(|s| s.is_valid()).unwrap_or(false)
    }

    /// Returns whether the latest received FCU is syncing: [`ForkchoiceStatus::Syncing`]
    #[allow(dead_code)]
    pub(crate) fn is_latest_syncing(&self) -> bool {
        self.latest_status().map(|s| s.is_syncing()).unwrap_or(false)
    }

    /// Returns whether the latest received FCU is syncing: [`ForkchoiceStatus::Invalid`]
    #[allow(dead_code)]
    pub(crate) fn is_latest_invalid(&self) -> bool {
        self.latest_status().map(|s| s.is_invalid()).unwrap_or(false)
    }

    /// Returns the last valid head hash.
    #[allow(dead_code)]
    pub(crate) fn last_valid_head(&self) -> Option<B256> {
        self.last_valid.as_ref().map(|s| s.head_block_hash)
    }

    /// Returns the head hash of the latest received FCU to which we need to sync.
    #[allow(dead_code)]
    pub(crate) fn sync_target(&self) -> Option<B256> {
        self.last_syncing.as_ref().map(|s| s.head_block_hash)
    }

    /// Returns the latest received `ForkchoiceState`.
    ///
    /// Caution: this can be invalid.
    pub const fn latest_state(&self) -> Option<ForkchoiceState> {
        self.last_valid
    }

    /// Returns the last valid `ForkchoiceState`.
    pub const fn last_valid_state(&self) -> Option<ForkchoiceState> {
        self.last_valid
    }

    /// Returns the last valid finalized hash.
    ///
    /// This will return [`None`], if either there is no valid finalized forkchoice state, or the
    /// finalized hash for the latest valid forkchoice state is zero.
    #[inline]
    pub fn last_valid_finalized(&self) -> Option<B256> {
        self.last_valid.and_then(|state| {
            // if the hash is zero then we should act like there is no finalized hash
            if state.finalized_block_hash.is_zero() {
                None
            } else {
                Some(state.finalized_block_hash)
            }
        })
    }

    /// Returns the last received `ForkchoiceState` to which we need to sync.
    pub const fn sync_target_state(&self) -> Option<ForkchoiceState> {
        self.last_syncing
    }

    /// Returns the sync target finalized hash.
    ///
    /// This will return [`None`], if either there is no sync target forkchoice state, or the
    /// finalized hash for the sync target forkchoice state is zero.
    #[inline]
    pub fn sync_target_finalized(&self) -> Option<B256> {
        self.last_syncing.and_then(|state| {
            // if the hash is zero then we should act like there is no finalized hash
            if state.finalized_block_hash.is_zero() {
                None
            } else {
                Some(state.finalized_block_hash)
            }
        })
    }

    /// Returns true if no forkchoice state has been received yet.
    pub const fn is_empty(&self) -> bool {
        self.latest.is_none()
    }
}

/// Represents a forkchoice update and tracks the status we assigned to it.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct ReceivedForkchoiceState {
    state: ForkchoiceState,
    status: ForkchoiceStatus,
}

/// A simplified representation of [`PayloadStatusEnum`] specifically for FCU.
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
    pub(crate) const fn is_valid(&self) -> bool {
        matches!(self, Self::Valid)
    }

    pub(crate) const fn is_invalid(&self) -> bool {
        matches!(self, Self::Invalid)
    }

    pub(crate) const fn is_syncing(&self) -> bool {
        matches!(self, Self::Syncing)
    }

    /// Converts the general purpose [`PayloadStatusEnum`] into a [`ForkchoiceStatus`].
    pub(crate) const fn from_payload_status(status: &PayloadStatusEnum) -> Self {
        match status {
            PayloadStatusEnum::Valid | PayloadStatusEnum::Accepted => {
                // `Accepted` is only returned on `newPayload`. It would be a valid state here.
                Self::Valid
            }
            PayloadStatusEnum::Invalid { .. } => Self::Invalid,
            PayloadStatusEnum::Syncing => Self::Syncing,
        }
    }
}

impl From<PayloadStatusEnum> for ForkchoiceStatus {
    fn from(status: PayloadStatusEnum) -> Self {
        Self::from_payload_status(&status)
    }
}

/// A helper type to check represent hashes of a [`ForkchoiceState`]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForkchoiceStateHash {
    /// Head hash of the [`ForkchoiceState`].
    Head(B256),
    /// Safe hash of the [`ForkchoiceState`].
    Safe(B256),
    /// Finalized hash of the [`ForkchoiceState`].
    Finalized(B256),
}

impl ForkchoiceStateHash {
    /// Tries to find a matching hash in the given [`ForkchoiceState`].
    pub(crate) fn find(state: &ForkchoiceState, hash: B256) -> Option<Self> {
        if state.head_block_hash == hash {
            Some(Self::Head(hash))
        } else if state.safe_block_hash == hash {
            Some(Self::Safe(hash))
        } else if state.finalized_block_hash == hash {
            Some(Self::Finalized(hash))
        } else {
            None
        }
    }

    /// Returns true if this is the head hash of the [`ForkchoiceState`]
    pub(crate) const fn is_head(&self) -> bool {
        matches!(self, Self::Head(_))
    }
}

impl AsRef<B256> for ForkchoiceStateHash {
    fn as_ref(&self) -> &B256 {
        match self {
            Self::Head(h) | Self::Safe(h) | Self::Finalized(h) => h,
        }
    }
}
