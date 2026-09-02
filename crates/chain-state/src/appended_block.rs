//! State-provider support for locally appended blocks.

use reth_errors::ProviderResult;
use reth_primitives_traits::NodePrimitives;
use reth_storage_api::{noop::NoopProvider, StateProviderBox};

use crate::ExecutedBlock;

/// Produces a state provider for a locally executed block on top of its parent state.
pub trait StateProviderWithAppendedBlock<N: NodePrimitives> {
    /// Returns a state provider for the post-state of `block`.
    fn state_provider_with_appended_block(
        &self,
        block: ExecutedBlock<N>,
    ) -> ProviderResult<StateProviderBox>;
}

impl<C: Send + Sync + 'static, N: NodePrimitives> StateProviderWithAppendedBlock<N>
    for NoopProvider<C, N>
{
    fn state_provider_with_appended_block(
        &self,
        _block: ExecutedBlock<N>,
    ) -> ProviderResult<StateProviderBox> {
        Ok(Box::new(self.clone()))
    }
}
