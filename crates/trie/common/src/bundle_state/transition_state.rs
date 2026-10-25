use super::TransitionAccount;
use alloy_primitives::{
    map::{AddressMap, HashMap},
    Address,
};

/// State of accounts in transition between transaction executions.
#[derive(Clone, Default, Debug, PartialEq, Eq)]
pub struct TransitionState {
    /// Block state account with account state
    pub transitions: AddressMap<TransitionAccount>,
}

impl TransitionState {
    /// Create new transition state containing one [`TransitionAccount`].
    pub fn single(address: Address, transition: TransitionAccount) -> Self {
        let mut transitions = HashMap::default();
        transitions.insert(address, transition);
        TransitionState { transitions }
    }

    /// Take the contents of this [`TransitionState`] and replace it with an
    /// empty one.
    ///
    /// See [core::mem::take].
    pub fn take(&mut self) -> TransitionState {
        core::mem::take(self)
    }

    /// Clear the transition state.
    pub fn clear(&mut self) {
        self.transitions.clear();
    }
}
