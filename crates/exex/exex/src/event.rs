use reth_primitives::BlockNumHash;

/// Events emitted by an `ExEx`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExExEvent {
    /// Highest block processed by the `ExEx`.
    ///
    /// The `ExEx` must guarantee that it will not require all earlier blocks in the future,
    /// meaning that Reth is allowed to prune them.
    ///
    /// On reorgs, it's possible for the height to go down.
    FinishedHeight(BlockNumHash),
}
