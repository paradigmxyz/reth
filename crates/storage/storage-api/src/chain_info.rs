use alloy_rpc_types_engine::ForkchoiceState;
use reth_primitives_traits::SealedHeader;

/// A type that can track updates related to fork choice updates.
pub trait CanonChainTracker: Send + Sync {
    /// The header type.
    type Header: Send + Sync;

    /// Notify the tracker about a received fork choice update.
    fn on_forkchoice_update_received(&self, update: &ForkchoiceState);

    /// Returns the last time a fork choice update was received from the CL
    /// ([`CanonChainTracker::on_forkchoice_update_received`])
    #[cfg(feature = "std")]
    fn last_received_update_timestamp(&self) -> Option<std::time::Instant>;

    /// Sets the canonical head of the chain.
    fn set_canonical_head(&self, header: SealedHeader<Self::Header>);

    /// Sets the safe block of the chain.
    fn set_safe(&self, header: SealedHeader<Self::Header>);

    /// Sets the finalized block of the chain.
    fn set_finalized(&self, header: SealedHeader<Self::Header>);
}
