use tokio::sync::mpsc::Receiver;

/// Consensus layer interface
#[async_trait::async_trait]
#[auto_impl::auto_impl(Arc)]
pub trait ClayerConsensus: Send + Sync + Clone {
    /// Returns pending consensus listener
    fn pending_consensus_listener(&self) -> Receiver<reth_primitives::Bytes>;
    /// push data received from network into cache
    fn push_cache(&self, data: reth_primitives::Bytes);
    /// pop data received from network out cache
    fn pop_cache(&self) -> Option<reth_primitives::Bytes>;
    /// broadcast consensus
    fn broadcast_consensus(&self, data: reth_primitives::Bytes);
}
