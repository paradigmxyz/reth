//! Drives one snap synchronization attempt: what it targets, and when it stops.

use crate::{SnapGeneration, SnapPivotPolicy, SnapSyncError};
use reth_storage_api::HeaderProvider;
use tokio_util::sync::CancellationToken;

/// One snap synchronization attempt, from the pivot it targets to the work it owns.
///
/// Targets come from the local chain, never from a peer: peers only supply state, which is
/// authenticated against the target's state root.
#[derive(Debug)]
pub struct SnapSyncSession {
    // Decides which blocks are eligible targets.
    policy: SnapPivotPolicy,
    // How far the attempt has got.
    state: SnapSyncSessionState,
    // Cancelled once, watched by whatever took the target.
    cancellation: CancellationToken,
}

impl SnapSyncSession {
    /// Creates a session waiting for its first eligible target.
    pub fn new(policy: SnapPivotPolicy) -> Self {
        Self {
            policy,
            state: SnapSyncSessionState::Waiting,
            cancellation: CancellationToken::new(),
        }
    }

    /// What the session is doing.
    pub const fn state(&self) -> &SnapSyncSessionState {
        &self.state
    }

    /// Pivot this session is anchored to, if it has one.
    pub const fn target(&self) -> Option<&SnapGeneration> {
        self.state.target()
    }

    /// Returns whether this session has been cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.cancellation.is_cancelled()
    }

    /// Selects a target under `head`, or waits while no block is eligible.
    ///
    /// A target no work has taken yet is replaced by a newer eligible one and dropped when none is
    /// eligible, since nothing authenticates against its root so far. Moving a target that work
    /// has taken is instead pivot advancement, which has to carry the downloaded state forward.
    pub fn select(
        &mut self,
        provider: &impl HeaderProvider,
        head: u64,
        finalized: Option<u64>,
    ) -> Result<&SnapSyncSessionState, SnapSyncError> {
        if matches!(self.state, SnapSyncSessionState::Waiting | SnapSyncSessionState::Selected(_)) {
            self.state = match self.policy.select(provider, head, finalized)? {
                Some(generation) => SnapSyncSessionState::Selected(generation),
                None => SnapSyncSessionState::Waiting,
            };
        }
        Ok(&self.state)
    }

    /// Hands the selected target, and the token to watch, to the work downloading against it.
    ///
    /// Taking the target is what starts it, so only the first caller gets one: a target already
    /// being downloaded has an owner, and a waiting or cancelled session has nothing to hand out.
    pub fn start(&mut self) -> Option<(SnapGeneration, CancellationToken)> {
        let SnapSyncSessionState::Selected(generation) = self.state else { return None };
        self.state = SnapSyncSessionState::Downloading(generation);
        Some((generation, self.cancellation.clone()))
    }

    /// Signals outstanding work to stop and ends the session.
    ///
    /// Terminal: a later attempt needs a new session.
    pub fn cancel(&mut self) {
        self.cancellation.cancel();
        self.state = SnapSyncSessionState::Cancelled;
    }
}

/// What a [`SnapSyncSession`] is doing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SnapSyncSessionState {
    /// No block is eligible yet, so the session holds no target.
    Waiting,
    /// A target is selected, but no work has taken it yet.
    Selected(SnapGeneration),
    /// Work is outstanding against the target.
    Downloading(SnapGeneration),
    /// The session was cancelled and its outstanding work signalled to stop.
    Cancelled,
}

impl SnapSyncSessionState {
    /// Target of this state, if it has one.
    pub const fn target(&self) -> Option<&SnapGeneration> {
        match self {
            Self::Selected(generation) | Self::Downloading(generation) => Some(generation),
            Self::Waiting | Self::Cancelled => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{chain, policy, provider_with};

    fn session() -> SnapSyncSession {
        SnapSyncSession::new(policy())
    }

    #[test]
    fn waits_while_no_block_is_eligible() {
        // The only block access list commitment is at the head, past the head distance.
        let provider = provider_with(chain(Some(3)));
        let mut session = session();

        assert_eq!(session.select(&provider, 3, None).unwrap(), &SnapSyncSessionState::Waiting);
        assert_eq!(session.target(), None);
        assert!(session.start().is_none());
    }

    #[test]
    fn selects_an_eligible_target() {
        let headers = chain(Some(0));
        let expected = headers[2].clone();
        let provider = provider_with(headers);
        let mut session = session();

        session.select(&provider, 3, None).unwrap();

        let target = session.target().unwrap();
        assert_eq!(target.target().number, 2);
        assert_eq!(target.target().hash, expected.hash_slow());
    }

    #[test]
    fn a_target_no_work_took_is_replaced_as_the_head_advances() {
        let provider = provider_with(chain(Some(0)));
        let mut session = session();

        session.select(&provider, 2, None).unwrap();
        assert_eq!(session.target().unwrap().target().number, 1);

        session.select(&provider, 3, None).unwrap();
        assert_eq!(session.target().unwrap().target().number, 2);
    }

    #[test]
    fn a_target_no_work_took_is_dropped_once_it_is_no_longer_eligible() {
        let provider = provider_with(chain(Some(0)));
        let mut session = session();
        session.select(&provider, 3, None).unwrap();

        // The head has run past the downloaded headers, so no candidate confirms the old target.
        session.select(&provider, 9, None).unwrap();

        assert_eq!(session.state(), &SnapSyncSessionState::Waiting);
        assert!(session.start().is_none());
    }

    #[test]
    fn a_target_work_took_is_kept() {
        let provider = provider_with(chain(Some(0)));
        let mut session = session();
        session.select(&provider, 2, None).unwrap();
        let (started, _) = session.start().unwrap();

        session.select(&provider, 3, None).unwrap();

        assert_eq!(session.state(), &SnapSyncSessionState::Downloading(started));
    }

    #[test]
    fn only_one_worker_takes_a_target() {
        let provider = provider_with(chain(Some(0)));
        let mut session = session();
        session.select(&provider, 3, None).unwrap();

        assert!(session.start().is_some());
        assert!(session.start().is_none());
    }

    #[test]
    fn cancellation_stops_outstanding_work() {
        let provider = provider_with(chain(Some(0)));
        let mut session = session();
        session.select(&provider, 3, None).unwrap();
        let (_, outstanding) = session.start().unwrap();

        session.cancel();

        assert!(outstanding.is_cancelled());
        assert!(session.is_cancelled());
        assert_eq!(session.state(), &SnapSyncSessionState::Cancelled);
        assert_eq!(session.target(), None);
    }

    #[test]
    fn a_cancelled_session_selects_nothing() {
        let provider = provider_with(chain(Some(0)));
        let mut session = session();
        session.cancel();

        assert_eq!(session.select(&provider, 3, None).unwrap(), &SnapSyncSessionState::Cancelled);
        assert!(session.start().is_none());
    }
}
