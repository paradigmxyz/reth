//! Noop impls for testing.

use crate::{
    CanonStateNotifications, CanonStateSubscriptions, ForkChoiceNotifications,
    ForkChoiceSubscriptions,
};
use reth_primitives_traits::NodePrimitives;
use reth_storage_api::noop::NoopProvider;
use tokio::sync::{broadcast, watch};

impl<C, N> CanonStateSubscriptions for NoopProvider<C, N>
where
    C: Send + Sync,
    N: NodePrimitives + Unpin + Default + Clone + 'static,
{
    fn subscribe_to_canonical_state(&self) -> CanonStateNotifications<N> {
        broadcast::channel(1).1
    }
}

impl<C: Send + Sync, N: NodePrimitives> ForkChoiceSubscriptions for NoopProvider<C, N> {
    type Header = N::BlockHeader;

    fn subscribe_safe_block(&self) -> ForkChoiceNotifications<N::BlockHeader> {
        let (_, rx) = watch::channel(None);
        ForkChoiceNotifications(rx)
    }

    fn subscribe_finalized_block(&self) -> ForkChoiceNotifications<N::BlockHeader> {
        let (_, rx) = watch::channel(None);
        ForkChoiceNotifications(rx)
    }
}
