#![allow(clippy::useless_conversion)]

#[cfg(any(not(feature = "scroll"), feature = "test-utils"))]
use reth_revm::{cached::CachedReadsDbMut, revm::CacheDB, DatabaseRef};
use reth_revm::{
    database::{EvmStateProvider, StateProviderDatabase},
    revm::State,
};
#[cfg(feature = "scroll")]
use reth_scroll_storage::ScrollStateProviderDatabase;

/// Finalize the execution of the type and return the output
pub trait FinalizeExecution {
    /// The output of the finalization.
    type Output;

    /// Finalize the state and return the output.
    fn finalize(&mut self) -> Self::Output;
}

#[cfg(feature = "scroll")]
impl<DB: EvmStateProvider> FinalizeExecution for State<ScrollStateProviderDatabase<DB>> {
    type Output = reth_revm::states::ScrollBundleState;

    fn finalize(&mut self) -> Self::Output {
        let bundle = self.take_bundle();
        (bundle, &self.database.post_execution_context).into()
    }
}

#[cfg(feature = "scroll")]
impl<DB: EvmStateProvider> FinalizeExecution for State<&mut ScrollStateProviderDatabase<DB>> {
    type Output = reth_revm::states::ScrollBundleState;

    fn finalize(&mut self) -> Self::Output {
        let bundle = self.take_bundle();
        (bundle, &self.database.post_execution_context).into()
    }
}

#[cfg(any(not(feature = "scroll"), feature = "test-utils"))]
impl<DB: EvmStateProvider> FinalizeExecution for State<StateProviderDatabase<DB>> {
    type Output = reth_revm::db::BundleState;

    fn finalize(&mut self) -> Self::Output {
        self.take_bundle().into()
    }
}

#[cfg(any(not(feature = "scroll"), feature = "test-utils"))]
impl<DB: EvmStateProvider> FinalizeExecution for State<&mut StateProviderDatabase<DB>> {
    type Output = reth_revm::db::BundleState;

    fn finalize(&mut self) -> Self::Output {
        self.take_bundle().into()
    }
}

#[cfg(any(not(feature = "scroll"), feature = "test-utils"))]
impl<DB: DatabaseRef> FinalizeExecution for State<CacheDB<DB>> {
    type Output = reth_revm::db::BundleState;

    fn finalize(&mut self) -> Self::Output {
        self.take_bundle().into()
    }
}

#[cfg(any(not(feature = "scroll"), feature = "test-utils"))]
impl<DB: DatabaseRef> FinalizeExecution for State<CachedReadsDbMut<'_, DB>> {
    type Output = reth_revm::db::BundleState;

    fn finalize(&mut self) -> Self::Output {
        self.take_bundle().into()
    }
}
