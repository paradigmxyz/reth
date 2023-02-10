use crate::StateProvider;
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
    _inner: Box<dyn StateProvider>,
    _phantom: PhantomData<&'a ()>,
}
