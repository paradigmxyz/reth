mod dev;
pub use dev::DEV_HARDFORKS;

use crate::{ForkCondition, ForkFilter, ForkId, Hardfork, Head};
#[cfg(feature = "std")]
use rustc_hash::FxHashMap;

use alloc::{boxed::Box, vec::Vec};

/// Generic trait over a set of ordered hardforks
#[auto_impl::auto_impl(&, Arc)]
pub trait Hardforks: Clone {
    /// Retrieves [`ForkCondition`] from `fork`. If `fork` is not present, returns
    /// [`ForkCondition::Never`].
    fn fork<H: Hardfork>(&self, fork: H) -> ForkCondition;

    /// Get an iterator of all hardforks with their respective activation conditions.
    fn forks_iter(&self) -> impl Iterator<Item = (&dyn Hardfork, ForkCondition)>;

    /// Convenience method to check if a fork is active at a given timestamp.
    fn is_fork_active_at_timestamp<H: Hardfork>(&self, fork: H, timestamp: u64) -> bool {
        self.fork(fork).active_at_timestamp(timestamp)
    }

    /// Convenience method to check if a fork is active at a given block number.
    fn is_fork_active_at_block<H: Hardfork>(&self, fork: H, block_number: u64) -> bool {
        self.fork(fork).active_at_block(block_number)
    }

    /// Compute the [`ForkId`] for the given [`Head`] following eip-6122 spec
    fn fork_id(&self, head: &Head) -> ForkId;

    /// Returns the [`ForkId`] for the last fork.
    ///
    /// NOTE: This returns the latest implemented [`ForkId`]. In many cases this will be the future
    /// [`ForkId`] on given network.
    fn latest_fork_id(&self) -> ForkId;

    /// Creates a [`ForkFilter`] for the block described by [Head].
    fn fork_filter(&self, head: Head) -> ForkFilter;

    /// Returns a [`ForkTimestamps`] helper for managing timestamp-based fork activations.
    fn fork_timestamps(&self) -> ForkTimestamps {
        ForkTimestamps::new(self)
    }
}

/// Ordered list of a chain hardforks that implement [`Hardfork`].
#[derive(Default, Clone, PartialEq, Eq)]
pub struct ChainHardforks {
    forks: Vec<(Box<dyn Hardfork>, ForkCondition)>,
    #[cfg(feature = "std")]
    map: FxHashMap<&'static str, ForkCondition>,
    #[cfg(not(feature = "std"))]
    map: alloc::collections::BTreeMap<&'static str, ForkCondition>,
}

impl ChainHardforks {
    /// Creates a new [`ChainHardforks`] from a list which **must be ordered** by activation.
    ///
    /// Equivalent Ethereum hardforks **must be included** as well.
    pub fn new(forks: Vec<(Box<dyn Hardfork>, ForkCondition)>) -> Self {
        let map = forks.iter().map(|(fork, condition)| (fork.name(), *condition)).collect();

        Self { forks, map }
    }

    /// Total number of hardforks.
    pub fn len(&self) -> usize {
        self.forks.len()
    }

    /// Checks if the fork list is empty.
    pub fn is_empty(&self) -> bool {
        self.forks.is_empty()
    }

    /// Retrieves [`ForkCondition`] from `fork`. If `fork` is not present, returns
    /// [`ForkCondition::Never`].
    pub fn fork<H: Hardfork>(&self, fork: H) -> ForkCondition {
        self.get(fork).unwrap_or_default()
    }

    /// Retrieves [`ForkCondition`] from `fork` if it exists, otherwise `None`.
    pub fn get<H: Hardfork>(&self, fork: H) -> Option<ForkCondition> {
        self.map.get(fork.name()).copied()
    }

    /// Retrieves the fork block number or timestamp from `fork` if it exists, otherwise `None`.
    pub fn fork_block<H: Hardfork>(&self, fork: H) -> Option<u64> {
        match self.fork(fork) {
            ForkCondition::Block(block) => Some(block),
            ForkCondition::TTD { fork_block, .. } => fork_block,
            ForkCondition::Timestamp(ts) => Some(ts),
            ForkCondition::Never => None,
        }
    }

    /// Get an iterator of all hardforks with their respective activation conditions.
    pub fn forks_iter(&self) -> impl Iterator<Item = (&dyn Hardfork, ForkCondition)> {
        self.forks.iter().map(|(f, b)| (&**f, *b))
    }

    /// Get last hardfork from the list.
    pub fn last(&self) -> Option<(Box<dyn Hardfork>, ForkCondition)> {
        self.forks.last().map(|(f, b)| (f.clone(), *b))
    }

    /// Convenience method to check if a fork is active at a given timestamp.
    pub fn is_fork_active_at_timestamp<H: Hardfork>(&self, fork: H, timestamp: u64) -> bool {
        self.fork(fork).active_at_timestamp(timestamp)
    }

    /// Convenience method to check if a fork is active at a given block number.
    pub fn is_fork_active_at_block<H: Hardfork>(&self, fork: H, block_number: u64) -> bool {
        self.fork(fork).active_at_block(block_number)
    }

    /// Inserts a fork with the given [`ForkCondition`], maintaining forks in ascending order
    /// based on the `Ord` implementation of [`ForkCondition`].
    ///
    /// If the fork already exists (regardless of its current condition type), it will be removed
    /// and re-inserted at the appropriate position based on the new condition.
    ///
    /// # Ordering Behavior
    ///
    /// Forks are ordered according to [`ForkCondition`]'s `Ord` implementation:
    /// - [`ForkCondition::Never`] comes first
    /// - [`ForkCondition::Block`] ordered by block number
    /// - [`ForkCondition::Timestamp`] ordered by timestamp value
    /// - [`ForkCondition::TTD`] ordered by total difficulty
    ///
    /// # Example
    ///
    /// ```ignore
    /// let mut forks = ChainHardforks::default();
    /// forks.insert(Fork::Frontier, ForkCondition::Block(0));
    /// forks.insert(Fork::Homestead, ForkCondition::Block(1_150_000));
    /// forks.insert(Fork::Cancun, ForkCondition::Timestamp(1710338135));
    ///
    /// // Forks are ordered: Frontier (Block 0), Homestead (Block 1150000), Cancun (Timestamp)
    /// ```
    pub fn insert<H: Hardfork>(&mut self, fork: H, condition: ForkCondition) {
        // Remove existing fork if it exists
        self.remove(&fork);

        // Find the correct position based on ForkCondition's Ord implementation
        let pos = self
            .forks
            .iter()
            .position(|(_, existing_condition)| *existing_condition > condition)
            .unwrap_or(self.forks.len());

        self.map.insert(fork.name(), condition);
        self.forks.insert(pos, (Box::new(fork), condition));
    }

    /// Extends the list with multiple forks, updating existing entries with new
    /// [`ForkCondition`]s if they already exist.
    ///
    /// Each fork is inserted using [`Self::insert`], maintaining proper ordering based on
    /// [`ForkCondition`]'s `Ord` implementation.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let mut forks = ChainHardforks::default();
    /// forks.extend([
    ///     (Fork::Homestead, ForkCondition::Block(1_150_000)),
    ///     (Fork::Frontier, ForkCondition::Block(0)),
    ///     (Fork::Cancun, ForkCondition::Timestamp(1710338135)),
    /// ]);
    ///
    /// // Forks will be automatically ordered: Frontier, Homestead, Cancun
    /// ```
    pub fn extend<H: Hardfork + Clone>(
        &mut self,
        forks: impl IntoIterator<Item = (H, ForkCondition)>,
    ) {
        for (fork, condition) in forks {
            self.insert(fork, condition);
        }
    }

    /// Removes `fork` from list.
    pub fn remove<H: Hardfork>(&mut self, fork: &H) {
        self.forks.retain(|(inner_fork, _)| inner_fork.name() != fork.name());
        self.map.remove(fork.name());
    }
}

impl core::fmt::Debug for ChainHardforks {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ChainHardforks")
            .field("0", &self.forks_iter().map(|(hf, cond)| (hf.name(), cond)).collect::<Vec<_>>())
            .finish()
    }
}

impl<T: Hardfork, const N: usize> From<[(T, ForkCondition); N]> for ChainHardforks {
    fn from(list: [(T, ForkCondition); N]) -> Self {
        Self::new(
            list.into_iter()
                .map(|(fork, cond)| (Box::new(fork) as Box<dyn Hardfork>, cond))
                .collect(),
        )
    }
}

/// Manages timestamp-based fork activations.
///
/// Contains a sorted, deduplicated list of fork timestamps and provides helpers
/// to determine current, next, and last fork timestamps relative to a given timestamp.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForkTimestamps {
    timestamps: Vec<u64>,
}

impl ForkTimestamps {
    /// Creates a new [`ForkTimestamps`] from a type implementing [`Hardforks`].
    ///
    /// Extracts all timestamp-based forks, sorts them, and deduplicates.
    pub fn new(hardforks: &impl Hardforks) -> Self {
        let mut timestamps =
            hardforks.forks_iter().filter_map(|(_, cond)| cond.as_timestamp()).collect::<Vec<_>>();
        timestamps.sort_unstable();
        timestamps.dedup();

        Self { timestamps }
    }

    /// Creates a new [`ForkTimestamps`] from an iterator of fork conditions.
    ///
    /// Extracts all timestamp-based forks, sorts them, and deduplicates.
    pub fn from_fork_conditions(
        forks: impl Iterator<Item = impl core::borrow::Borrow<ForkCondition>>,
    ) -> Self {
        let mut timestamps =
            forks.filter_map(|cond| cond.borrow().as_timestamp()).collect::<Vec<_>>();
        timestamps.sort_unstable();
        timestamps.dedup();

        Self { timestamps }
    }

    /// Returns the current active fork timestamp at the given timestamp.
    ///
    /// Returns the highest fork timestamp that is less than or equal to the given timestamp.
    pub fn current(&self, timestamp: u64) -> Option<u64> {
        self.current_with_index(timestamp).map(|(_, ts)| ts)
    }

    /// Returns the current active fork timestamp with its index at the given timestamp.
    ///
    /// The index can be used to efficiently retrieve next/last forks.
    pub fn current_with_index(&self, timestamp: u64) -> Option<(usize, u64)> {
        self.timestamps
            .iter()
            // find the first timestamp that's greater that the given timestamp
            .position(|ts| &timestamp < ts)
            // get the previous one, which is the currently active timestamp
            .and_then(|idx| idx.checked_sub(1))
            .or_else(|| self.timestamps.len().checked_sub(1))
            .and_then(|idx| self.timestamps.get(idx).map(|ts| (idx, *ts)))
    }

    /// Returns the next scheduled fork timestamp after the given timestamp.
    pub fn next(&self, timestamp: u64) -> Option<u64> {
        self.current_with_index(timestamp)
            .and_then(|(idx, _)| self.timestamps.get(idx + 1).copied())
    }

    /// Returns the last (final) fork timestamp in the schedule.
    pub fn last(&self) -> Option<u64> {
        self.timestamps.last().copied()
    }

    /// Returns true if there are no timestamp-based forks.
    pub const fn is_empty(&self) -> bool {
        self.timestamps.is_empty()
    }

    /// Returns the number of timestamp-based forks.
    pub const fn len(&self) -> usize {
        self.timestamps.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_hardforks::hardfork;

    hardfork!(AHardfork { A1, A2, A3 });
    hardfork!(BHardfork { B1, B2 });

    #[test]
    fn add_hardforks() {
        let mut forks = ChainHardforks::default();
        forks.insert(AHardfork::A1, ForkCondition::Block(1));
        forks.insert(BHardfork::B1, ForkCondition::Block(1));
        assert_eq!(forks.len(), 2);
        forks.is_fork_active_at_block(AHardfork::A1, 1);
        forks.is_fork_active_at_block(BHardfork::B1, 1);
    }

    #[test]
    fn insert_maintains_fork_order() {
        let mut forks = ChainHardforks::default();

        // Insert forks in random order
        forks.insert(BHardfork::B1, ForkCondition::Timestamp(2000));
        forks.insert(AHardfork::A1, ForkCondition::Block(100));
        forks.insert(AHardfork::A2, ForkCondition::Block(50));
        forks.insert(BHardfork::B2, ForkCondition::Timestamp(1000));

        assert_eq!(forks.len(), 4);

        let fork_list: Vec<_> = forks.forks_iter().collect();

        // Verify ordering: Block conditions come before Timestamp conditions
        // and within each type, they're ordered by value
        assert_eq!(fork_list[0].0.name(), "A2");
        assert_eq!(fork_list[0].1, ForkCondition::Block(50));
        assert_eq!(fork_list[1].0.name(), "A1");
        assert_eq!(fork_list[1].1, ForkCondition::Block(100));
        assert_eq!(fork_list[2].0.name(), "B2");
        assert_eq!(fork_list[2].1, ForkCondition::Timestamp(1000));
        assert_eq!(fork_list[3].0.name(), "B1");
        assert_eq!(fork_list[3].1, ForkCondition::Timestamp(2000));
    }

    #[test]
    fn insert_replaces_and_reorders_existing_fork() {
        let mut forks = ChainHardforks::default();

        // Insert initial forks
        forks.insert(AHardfork::A1, ForkCondition::Block(100));
        forks.insert(BHardfork::B1, ForkCondition::Block(200));
        forks.insert(AHardfork::A2, ForkCondition::Timestamp(1000));

        assert_eq!(forks.len(), 3);

        // Update A1 from Block to Timestamp - should move it after B1
        forks.insert(AHardfork::A1, ForkCondition::Timestamp(500));
        assert_eq!(forks.len(), 3);

        let fork_list: Vec<_> = forks.forks_iter().collect();

        // Verify new ordering
        assert_eq!(fork_list[0].0.name(), "B1");
        assert_eq!(fork_list[0].1, ForkCondition::Block(200));
        assert_eq!(fork_list[1].0.name(), "A1");
        assert_eq!(fork_list[1].1, ForkCondition::Timestamp(500));
        assert_eq!(fork_list[2].0.name(), "A2");
        assert_eq!(fork_list[2].1, ForkCondition::Timestamp(1000));

        // Update A1 timestamp to move it after A2
        forks.insert(AHardfork::A1, ForkCondition::Timestamp(2000));
        assert_eq!(forks.len(), 3);

        let fork_list: Vec<_> = forks.forks_iter().collect();

        assert_eq!(fork_list[0].0.name(), "B1");
        assert_eq!(fork_list[0].1, ForkCondition::Block(200));
        assert_eq!(fork_list[1].0.name(), "A2");
        assert_eq!(fork_list[1].1, ForkCondition::Timestamp(1000));
        assert_eq!(fork_list[2].0.name(), "A1");
        assert_eq!(fork_list[2].1, ForkCondition::Timestamp(2000));
    }

    #[test]
    fn extend_maintains_order() {
        let mut forks = ChainHardforks::default();

        // Use extend to insert multiple forks at once in random order
        forks.extend([
            (AHardfork::A1, ForkCondition::Block(100)),
            (AHardfork::A2, ForkCondition::Timestamp(1000)),
        ]);
        forks.extend([(BHardfork::B1, ForkCondition::Timestamp(2000))]);

        assert_eq!(forks.len(), 3);

        let fork_list: Vec<_> = forks.forks_iter().collect();

        // Verify ordering is maintained
        assert_eq!(fork_list[0].0.name(), "A1");
        assert_eq!(fork_list[0].1, ForkCondition::Block(100));
        assert_eq!(fork_list[1].0.name(), "A2");
        assert_eq!(fork_list[1].1, ForkCondition::Timestamp(1000));
        assert_eq!(fork_list[2].0.name(), "B1");
        assert_eq!(fork_list[2].1, ForkCondition::Timestamp(2000));

        // Extend again with an update to A2
        forks.extend([(AHardfork::A2, ForkCondition::Timestamp(3000))]);
        assert_eq!(forks.len(), 3);

        let fork_list: Vec<_> = forks.forks_iter().collect();

        assert_eq!(fork_list[0].0.name(), "A1");
        assert_eq!(fork_list[1].0.name(), "B1");
        assert_eq!(fork_list[2].0.name(), "A2");
        assert_eq!(fork_list[2].1, ForkCondition::Timestamp(3000));
    }

    #[test]
    fn fork_timestamps_creation() {
        let mut forks = ChainHardforks::default();
        forks.insert(AHardfork::A1, ForkCondition::Block(100));
        forks.insert(AHardfork::A2, ForkCondition::Timestamp(1000));
        forks.insert(BHardfork::B1, ForkCondition::Timestamp(2000));
        forks.insert(BHardfork::B2, ForkCondition::Timestamp(1000)); // Duplicate timestamp

        let fork_timestamps =
            ForkTimestamps::from_fork_conditions(forks.forks_iter().map(|(_, cond)| cond));

        // Should have 2 unique timestamps (1000, 2000), block-based fork excluded
        assert_eq!(fork_timestamps.len(), 2);
        assert!(!fork_timestamps.is_empty());
    }

    #[test]
    fn fork_timestamps_current() {
        let mut forks = ChainHardforks::default();
        forks.insert(AHardfork::A1, ForkCondition::Timestamp(1000));
        forks.insert(AHardfork::A2, ForkCondition::Timestamp(2000));
        forks.insert(AHardfork::A3, ForkCondition::Timestamp(3000));

        let fork_timestamps =
            ForkTimestamps::from_fork_conditions(forks.forks_iter().map(|(_, cond)| cond));

        // At first fork
        assert_eq!(fork_timestamps.current(1000), Some(1000));

        // Between forks
        assert_eq!(fork_timestamps.current(1500), Some(1000));
        assert_eq!(fork_timestamps.current(2500), Some(2000));

        // At last fork
        assert_eq!(fork_timestamps.current(3000), Some(3000));

        // After last fork
        assert_eq!(fork_timestamps.current(4000), Some(3000));
    }

    #[test]
    fn fork_timestamps_next() {
        let mut forks = ChainHardforks::default();
        forks.insert(AHardfork::A1, ForkCondition::Timestamp(1000));
        forks.insert(AHardfork::A2, ForkCondition::Timestamp(2000));
        forks.insert(AHardfork::A3, ForkCondition::Timestamp(3000));

        let fork_timestamps =
            ForkTimestamps::from_fork_conditions(forks.forks_iter().map(|(_, cond)| cond));

        // Before first fork - no current, so no next
        assert_eq!(fork_timestamps.next(500), None);

        // At first fork - next is 2000
        assert_eq!(fork_timestamps.next(1000), Some(2000));

        // Between forks
        assert_eq!(fork_timestamps.next(1500), Some(2000));
        assert_eq!(fork_timestamps.next(2500), Some(3000));

        // At last fork - no next
        assert_eq!(fork_timestamps.next(3000), None);

        // After last fork - no next
        assert_eq!(fork_timestamps.next(4000), None);
    }

    #[test]
    fn fork_timestamps_last() {
        let mut forks = ChainHardforks::default();
        forks.insert(AHardfork::A1, ForkCondition::Timestamp(1000));
        forks.insert(AHardfork::A2, ForkCondition::Timestamp(2000));
        forks.insert(AHardfork::A3, ForkCondition::Timestamp(3000));

        let fork_timestamps =
            ForkTimestamps::from_fork_conditions(forks.forks_iter().map(|(_, cond)| cond));

        assert_eq!(fork_timestamps.last(), Some(3000));
    }

    #[test]
    fn fork_timestamps_empty() {
        let forks = ChainHardforks::default();
        let fork_timestamps =
            ForkTimestamps::from_fork_conditions(forks.forks_iter().map(|(_, cond)| cond));

        assert!(fork_timestamps.is_empty());
        assert_eq!(fork_timestamps.len(), 0);
        assert_eq!(fork_timestamps.current(1000), None);
        assert_eq!(fork_timestamps.next(1000), None);
        assert_eq!(fork_timestamps.last(), None);
    }

    #[test]
    fn fork_timestamps_with_index() {
        let mut forks = ChainHardforks::default();
        forks.insert(AHardfork::A1, ForkCondition::Timestamp(1000));
        forks.insert(AHardfork::A2, ForkCondition::Timestamp(2000));
        forks.insert(AHardfork::A3, ForkCondition::Timestamp(3000));

        let fork_timestamps =
            ForkTimestamps::from_fork_conditions(forks.forks_iter().map(|(_, cond)| cond));

        assert_eq!(fork_timestamps.current_with_index(1500), Some((0, 1000)));
        assert_eq!(fork_timestamps.current_with_index(2500), Some((1, 2000)));
        assert_eq!(fork_timestamps.current_with_index(3500), Some((2, 3000)));
    }
}
