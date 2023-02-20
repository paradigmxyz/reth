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
pub struct ChainState<'a> {
    inner: Box<dyn StateProvider>,
    _phantom: PhantomData<&'a ()>,
}

// == impl ChainState ===

impl<'a> ChainState<'a> {
    /// Wraps the given [StateProvider]
    pub fn new(inner: Box<dyn StateProvider>) -> Self {
        Self { inner, _phantom: Default::default() }
    }

    /// Returns a new provider that takes the `TX` as reference
    #[inline(always)]
    fn as_ref(&self) -> impl StateProvider + '_ {
        &*self.inner
    }
}

// Delegates all provider impls to the boxed [StateProvider]
delegate_provider_impls!(ChainState<'a>);

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_state_provider<T: StateProvider>() {}
    #[allow(unused)]
    #[allow(clippy::extra_unused_lifetimes)]
    fn assert_chain_state_provider<'txn>() {
        assert_state_provider::<ChainState<'txn>>();
    }
}
