use crate::{
    providers::state::macros::delegate_provider_impls, AccountProvider, BlockHashProvider,
    StateProvider,
};
use std::marker::PhantomData;

/// A type that can access the state at a specific access point (block number or tag)
///
/// Depending on the desired access point, the state must be accessed differently. For example, the
/// "Latest" state is stored in a different location than previous blocks. And the "Pending" state
/// is accessed differently than the "Latest" state.
///
/// This unifies [StateProvider] access when the caller does not know or care where the state is
/// being accessed from, e.g. in RPC where the requested access point may be
/// `Pending|Latest|Number|Hash`.
///
/// Note: The lifetime of this type is limited by the type that created it.
pub struct ChainState<'a, S: StateProvider> {
    inner: Box<S>,
    _phantom: PhantomData<&'a ()>,
}

// == impl ChainState ===

impl<'a, S: StateProvider> ChainState<'a, S> {
    /// Wraps the given [StateProvider]
    pub fn new(inner: Box<S>) -> Self {
        Self { inner, _phantom: Default::default() }
    }

    /// Returns a new provider that takes the `TX` as reference
    #[inline(always)]
    fn as_ref(&self) -> impl StateProvider + '_ {
        &*self.inner
    }
}

// Delegates all provider impls to the boxed [StateProvider]
delegate_provider_impls!(ChainState<'a, S> where [S: StateProvider]);

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_state_provider<S: StateProvider>() {}
    #[allow(unused)]
    fn assert_chain_state_provider<'txn, S: StateProvider>() {
        assert_state_provider::<ChainState<'txn, S>>();
    }
}
