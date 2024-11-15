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
        self.latest_status().map_or(false, |s| s.is_valid())
    }

    /// Returns whether the latest received FCU is syncing: [`ForkchoiceStatus::Syncing`]
    #[allow(dead_code)]
    pub(crate) fn is_latest_syncing(&self) -> bool {
        self.latest_status().map_or(false, |s| s.is_syncing())
    }

    /// Returns whether the latest received FCU is syncing: [`ForkchoiceStatus::Invalid`]
    #[allow(dead_code)]
    pub fn is_latest_invalid(&self) -> bool {
        self.latest_status().map_or(false, |s| s.is_invalid())
    }

    /// Returns the last valid head hash.
    #[allow(dead_code)]
    pub fn last_valid_head(&self) -> Option<B256> {
        self.last_valid.as_ref().map(|s| s.head_block_hash)
    }

    /// Returns the head hash of the latest received FCU to which we need to sync.
    #[allow(dead_code)]
    pub(crate) fn sync_target(&self) -> Option<B256> {
        self.last_syncing.as_ref().map(|s| s.head_block_hash)
    }

    /// Returns the latest received [`ForkchoiceState`].
    ///
    /// Caution: this can be invalid.
    pub const fn latest_state(&self) -> Option<ForkchoiceState> {
        self.last_valid
    }

    /// Returns the last valid [`ForkchoiceState`].
    pub const fn last_valid_state(&self) -> Option<ForkchoiceState> {
        self.last_valid
    }

    /// Returns the last valid finalized hash.
    ///
    /// This will return [`None`]:
    /// - If either there is no valid finalized forkchoice state,
    /// - Or the finalized hash for the latest valid forkchoice state is zero.
    #[inline]
    pub fn last_valid_finalized(&self) -> Option<B256> {
        self.last_valid
            .filter(|state| !state.finalized_block_hash.is_zero())
            .map(|state| state.finalized_block_hash)
    }

    /// Returns the last received `ForkchoiceState` to which we need to sync.
    pub const fn sync_target_state(&self) -> Option<ForkchoiceState> {
        self.last_syncing
    }

    /// Returns the sync target finalized hash.
    ///
    /// This will return [`None`]:
    /// - If either there is no sync target forkchoice state,
    /// - Or the finalized hash for the sync target forkchoice state is zero.
    #[inline]
    pub fn sync_target_finalized(&self) -> Option<B256> {
        self.last_syncing
            .filter(|state| !state.finalized_block_hash.is_zero())
            .map(|state| state.finalized_block_hash)
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
    /// Returns `true` if the forkchoice state is [`ForkchoiceStatus::Valid`].
    pub const fn is_valid(&self) -> bool {
        matches!(self, Self::Valid)
    }

    /// Returns `true` if the forkchoice state is [`ForkchoiceStatus::Invalid`].
    pub const fn is_invalid(&self) -> bool {
        matches!(self, Self::Invalid)
    }

    /// Returns `true` if the forkchoice state is [`ForkchoiceStatus::Syncing`].
    pub const fn is_syncing(&self) -> bool {
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
    pub fn find(state: &ForkchoiceState, hash: B256) -> Option<Self> {
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
    pub const fn is_head(&self) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_forkchoice_state_tracker_set_latest_valid() {
        let mut tracker = ForkchoiceStateTracker::default();

        // Latest state is None
        assert!(tracker.latest_status().is_none());

        // Create a valid ForkchoiceState
        let state = ForkchoiceState {
            head_block_hash: B256::from_slice(&[1; 32]),
            safe_block_hash: B256::from_slice(&[2; 32]),
            finalized_block_hash: B256::from_slice(&[3; 32]),
        };
        let status = ForkchoiceStatus::Valid;

        tracker.set_latest(state, status);

        // Assert that the latest state is set
        assert!(tracker.latest.is_some());
        assert_eq!(tracker.latest.as_ref().unwrap().state, state);

        // Assert that last valid state is updated
        assert!(tracker.last_valid.is_some());
        assert_eq!(tracker.last_valid.as_ref().unwrap(), &state);

        // Assert that last syncing state is None
        assert!(tracker.last_syncing.is_none());

        // Test when there is a latest status and it is valid
        assert_eq!(tracker.latest_status(), Some(ForkchoiceStatus::Valid));
    }

    #[test]
    fn test_forkchoice_state_tracker_set_latest_syncing() {
        let mut tracker = ForkchoiceStateTracker::default();

        // Create a syncing ForkchoiceState
        let state = ForkchoiceState {
            head_block_hash: B256::from_slice(&[1; 32]),
            safe_block_hash: B256::from_slice(&[2; 32]),
            finalized_block_hash: B256::from_slice(&[0; 32]), // Zero to simulate not finalized
        };
        let status = ForkchoiceStatus::Syncing;

        tracker.set_latest(state, status);

        // Assert that the latest state is set
        assert!(tracker.latest.is_some());
        assert_eq!(tracker.latest.as_ref().unwrap().state, state);

        // Assert that last valid state is None since the status is syncing
        assert!(tracker.last_valid.is_none());

        // Assert that last syncing state is updated
        assert!(tracker.last_syncing.is_some());
        assert_eq!(tracker.last_syncing.as_ref().unwrap(), &state);

        // Test when there is a latest status and it is syncing
        assert_eq!(tracker.latest_status(), Some(ForkchoiceStatus::Syncing));
    }

    #[test]
    fn test_forkchoice_state_tracker_set_latest_invalid() {
        let mut tracker = ForkchoiceStateTracker::default();

        // Create an invalid ForkchoiceState
        let state = ForkchoiceState {
            head_block_hash: B256::from_slice(&[1; 32]),
            safe_block_hash: B256::from_slice(&[2; 32]),
            finalized_block_hash: B256::from_slice(&[3; 32]),
        };
        let status = ForkchoiceStatus::Invalid;

        tracker.set_latest(state, status);

        // Assert that the latest state is set
        assert!(tracker.latest.is_some());
        assert_eq!(tracker.latest.as_ref().unwrap().state, state);

        // Assert that last valid state is None since the status is invalid
        assert!(tracker.last_valid.is_none());

        // Assert that last syncing state is None since the status is invalid
        assert!(tracker.last_syncing.is_none());

        // Test when there is a latest status and it is invalid
        assert_eq!(tracker.latest_status(), Some(ForkchoiceStatus::Invalid));
    }

    #[test]
    fn test_forkchoice_state_tracker_sync_target() {
        let mut tracker = ForkchoiceStateTracker::default();

        // Test when there is no last syncing state (should return None)
        assert!(tracker.sync_target().is_none());

        // Set a last syncing forkchoice state
        let state = ForkchoiceState {
            head_block_hash: B256::from_slice(&[1; 32]),
            safe_block_hash: B256::from_slice(&[2; 32]),
            finalized_block_hash: B256::from_slice(&[3; 32]),
        };
        tracker.last_syncing = Some(state);

        // Test when the last syncing state is set (should return the head block hash)
        assert_eq!(tracker.sync_target(), Some(B256::from_slice(&[1; 32])));
    }

    #[test]
    fn test_forkchoice_state_tracker_last_valid_finalized() {
        let mut tracker = ForkchoiceStateTracker::default();

        // No valid finalized state (should return None)
        assert!(tracker.last_valid_finalized().is_none());

        // Valid finalized state, but finalized hash is zero (should return None)
        let zero_finalized_state = ForkchoiceState {
            head_block_hash: B256::ZERO,
            safe_block_hash: B256::ZERO,
            finalized_block_hash: B256::ZERO, // Zero finalized hash
        };
        tracker.last_valid = Some(zero_finalized_state);
        assert!(tracker.last_valid_finalized().is_none());

        // Valid finalized state with non-zero finalized hash (should return finalized hash)
        let valid_finalized_state = ForkchoiceState {
            head_block_hash: B256::from_slice(&[1; 32]),
            safe_block_hash: B256::from_slice(&[2; 32]),
            finalized_block_hash: B256::from_slice(&[123; 32]), // Non-zero finalized hash
        };
        tracker.last_valid = Some(valid_finalized_state);
        assert_eq!(tracker.last_valid_finalized(), Some(B256::from_slice(&[123; 32])));

        // Reset the last valid state to None
        tracker.last_valid = None;
        assert!(tracker.last_valid_finalized().is_none());
    }

    #[test]
    fn test_forkchoice_state_tracker_sync_target_finalized() {
        let mut tracker = ForkchoiceStateTracker::default();

        // No sync target state (should return None)
        assert!(tracker.sync_target_finalized().is_none());

        // Sync target state with finalized hash as zero (should return None)
        let zero_finalized_sync_target = ForkchoiceState {
            head_block_hash: B256::from_slice(&[1; 32]),
            safe_block_hash: B256::from_slice(&[2; 32]),
            finalized_block_hash: B256::ZERO, // Zero finalized hash
        };
        tracker.last_syncing = Some(zero_finalized_sync_target);
        assert!(tracker.sync_target_finalized().is_none());

        // Sync target state with non-zero finalized hash (should return the hash)
        let valid_sync_target = ForkchoiceState {
            head_block_hash: B256::from_slice(&[1; 32]),
            safe_block_hash: B256::from_slice(&[2; 32]),
            finalized_block_hash: B256::from_slice(&[22; 32]), // Non-zero finalized hash
        };
        tracker.last_syncing = Some(valid_sync_target);
        assert_eq!(tracker.sync_target_finalized(), Some(B256::from_slice(&[22; 32])));

        // Reset the last sync target state to None
        tracker.last_syncing = None;
        assert!(tracker.sync_target_finalized().is_none());
    }

    #[test]
    fn test_forkchoice_state_tracker_is_empty() {
        let mut forkchoice = ForkchoiceStateTracker::default();

        // Initially, no forkchoice state has been received, so it should be empty.
        assert!(forkchoice.is_empty());

        // After setting a forkchoice state, it should no longer be empty.
        forkchoice.set_latest(ForkchoiceState::default(), ForkchoiceStatus::Valid);
        assert!(!forkchoice.is_empty());

        // Reset the forkchoice latest, it should be empty again.
        forkchoice.latest = None;
        assert!(forkchoice.is_empty());
    }

    #[test]
    fn test_forkchoice_state_hash_find() {
        // Define example hashes
        let head_hash = B256::random();
        let safe_hash = B256::random();
        let finalized_hash = B256::random();
        let non_matching_hash = B256::random();

        // Create a ForkchoiceState with specific hashes
        let state = ForkchoiceState {
            head_block_hash: head_hash,
            safe_block_hash: safe_hash,
            finalized_block_hash: finalized_hash,
        };

        // Test finding the head hash
        assert_eq!(
            ForkchoiceStateHash::find(&state, head_hash),
            Some(ForkchoiceStateHash::Head(head_hash))
        );

        // Test finding the safe hash
        assert_eq!(
            ForkchoiceStateHash::find(&state, safe_hash),
            Some(ForkchoiceStateHash::Safe(safe_hash))
        );

        // Test finding the finalized hash
        assert_eq!(
            ForkchoiceStateHash::find(&state, finalized_hash),
            Some(ForkchoiceStateHash::Finalized(finalized_hash))
        );

        // Test with a hash that doesn't match any of the hashes in ForkchoiceState
        assert_eq!(ForkchoiceStateHash::find(&state, non_matching_hash), None);
    }
}
