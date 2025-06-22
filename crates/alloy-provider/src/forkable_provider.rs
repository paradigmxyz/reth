use crate::forkdb::ForkDb;
use reth_primitives::EthPrimitives;
use reth_provider::providers::{BlockchainProvider, ProviderNodeTypes};
use std::sync::{Arc, RwLock};

#[derive(Debug)]
/// A blockchain provider that supports forking functionality.
///
/// `ForkableBlockchainProvider` wraps a `BlockchainProvider` and a fork database,
/// allowing for operations that simulate forked chains (e.g., snapshots, state rewinds).
pub struct ForkableBlockchainProvider<N>
where
    N: ProviderNodeTypes<Primitives = EthPrimitives>,
{
    /// The underlying blockchain provider used to fetch and interact with on-chain data.
    pub provider: BlockchainProvider<N>,
    /// An optional ForkDB used to track and manage forked blockchain state.
    ///
    /// Wrapped in an `Arc<RwLock<...>>` to allow concurrent access across threads.
    pub fork_db: Arc<RwLock<Option<ForkDb>>>,
}
