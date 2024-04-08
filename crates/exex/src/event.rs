use reth_primitives::BlockNumber;

/// Events emitted by an ExEx.
#[derive(Debug)]
pub enum ExExEvent {
    /// Highest block processed by the ExEx.
    ///
    /// The ExEx must guarantee that it will not require all earlier blocks in the future, meaning
    /// that Reth is allowed to prune them.
    ///
    /// On reorgs, it's possible for the height to go down.
    FinishedHeight(BlockNumber),
}
