/// Result of stepping once with walker and pruning destination.
#[derive(Debug)]
pub enum PruneStepResult {
    /// Walker has no further destination.
    Finished,
    /// Data was pruned at current destination.
    MaybeMoreData,
}

impl PruneStepResult {
    /// Returns `true` if there are no more entries to prune in given range.
    pub fn is_finished(&self) -> bool {
        matches!(self, Self::Finished)
    }
}
