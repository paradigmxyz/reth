//! Placeholder precompile cache while payload prewarming is parked for evm2.

use std::marker::PhantomData;

/// Placeholder for the precompile cache while prewarm execution is parked.
#[derive(Debug, Clone, Default)]
pub struct PrecompileCacheMap<S>(PhantomData<fn() -> S>);
