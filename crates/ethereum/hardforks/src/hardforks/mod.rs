mod dev;
pub use dev::DEV_HARDFORKS;

use crate::{ForkCondition, ForkFilter, ForkId, Hardfork, Head};
#[cfg(feature = "std")]
use rustc_hash::FxHashMap;
#[cfg(feature = "std")]
use std::collections::hash_map::Entry;

#[cfg(not(feature = "std"))]
use alloc::collections::btree_map::Entry;
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

    /// Inserts a [`ForkCondition::Timestamp`] while maintaining timestamp-based forks in
    /// ascending order by timestamp value.
    ///
    /// If the fork already exists (regardless of its current condition type), it will move to the
    /// appropriate position.
    ///
    /// # Ordering Behavior
    ///
    /// - Timestamp-based forks are inserted after all other fork condition types
    ///   ([`ForkCondition::Block`], [`ForkCondition::TTD`], etc.)
    /// - Among timestamp-based forks, they are ordered by timestamp in ascending order
    /// - If no timestamp-based forks exist, the fork is appended to the end
    ///
    /// # Example
    ///
    /// ```ignore
    /// let mut forks = ChainHardforks::default();
    /// forks.insert(Fork::A, ForkCondition::Block(100));
    /// forks.insert_at_timestamp(Fork::B, 2000);
    /// forks.insert_at_timestamp(Fork::C, 1000);
    ///
    /// // Order: Fork::A (Block), Fork::C (Timestamp 1000), Fork::B (Timestamp 2000)
    /// ```
    pub fn insert_at_timestamp<H: Hardfork>(&mut self, fork: H, timestamp: u64) {
        // first remove existing hardfork
        if self.map.remove(&fork.name()).is_some() {
            self.forks.retain(|(inner_fork, _)| inner_fork.name() != fork.name());
        }

        // find the correct position
        let new_condition = ForkCondition::Timestamp(timestamp);

        // Find the position: among Timestamp conditions, insert before the first one with a higher
        // timestamp
        let pos = self
            .forks
            .iter()
            .position(|(_, condition)| match condition {
                ForkCondition::Timestamp(ts) => *ts > timestamp,
                _ => false,
            })
            .unwrap_or(self.forks.len());

        self.map.insert(fork.name(), new_condition);
        self.forks.insert(pos, (Box::new(fork), new_condition));
    }

    /// Inserts `fork` into list, updating with a new [`ForkCondition`] if it already exists.
    ///
    /// Note: This only appends the the given condition without re-ordering, see also
    /// [`Self::insert_at_timestamp`].
    pub fn insert<H: Hardfork>(&mut self, fork: H, condition: ForkCondition) {
        match self.map.entry(fork.name()) {
            Entry::Occupied(mut entry) => {
                *entry.get_mut() = condition;
                if let Some((_, inner)) =
                    self.forks.iter_mut().find(|(inner, _)| inner.name() == fork.name())
                {
                    *inner = condition;
                }
            }
            Entry::Vacant(entry) => {
                entry.insert(condition);
                self.forks.push((Box::new(fork), condition));
            }
        }
    }

    /// Extends the list with multiple forks, updating existing entries with new
    /// [`ForkCondition`]s if they already exist.
    ///
    /// Note: This only appends the the given condition without re-ordering, see also
    /// [`Self::insert_at_timestamp`].
    pub fn extend<H: Hardfork>(&mut self, forks: impl IntoIterator<Item = (H, ForkCondition)>) {
        for (fork, condition) in forks {
            self.insert(fork, condition);
        }
    }

    /// Removes `fork` from list.
    pub fn remove<H: Hardfork>(&mut self, fork: H) {
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
    fn insert_at_timestamp_maintains_order() {
        let mut forks = ChainHardforks::default();

        // Insert forks at different timestamps
        forks.insert_at_timestamp(BHardfork::B1, 2000);
        forks.insert_at_timestamp(AHardfork::A1, 1000);

        assert_eq!(forks.len(), 2);

        // Verify they are ordered by timestamp
        let fork_list: Vec<_> = forks.forks_iter().collect();
        assert_eq!(fork_list[0].0.name(), "A1");
        assert_eq!(fork_list[0].1, ForkCondition::Timestamp(1000));
        assert_eq!(fork_list[1].0.name(), "B1");
        assert_eq!(fork_list[1].1, ForkCondition::Timestamp(2000));

        // Verify fork conditions
        assert_eq!(forks.fork(AHardfork::A1), ForkCondition::Timestamp(1000));
        assert_eq!(forks.fork(BHardfork::B1), ForkCondition::Timestamp(2000));

        // Update existing fork with new timestamp
        forks.insert_at_timestamp(AHardfork::A1, 3000);
        assert_eq!(forks.len(), 2);

        // Verify updated order
        let fork_list: Vec<_> = forks.forks_iter().collect();
        assert_eq!(fork_list[0].0.name(), "B1");
        assert_eq!(fork_list[0].1, ForkCondition::Timestamp(2000));
        assert_eq!(fork_list[1].0.name(), "A1");
        assert_eq!(fork_list[1].1, ForkCondition::Timestamp(3000));
    }

    #[test]
    fn insert_at_timestamp_with_mixed_conditions() {
        let mut forks = ChainHardforks::default();

        // Insert block-based forks first
        forks.insert(AHardfork::A1, ForkCondition::Block(100));
        forks.insert(BHardfork::B1, ForkCondition::Block(200));

        // Insert timestamp-based forks - should come after block-based ones
        forks.insert_at_timestamp(AHardfork::A2, 2000);
        forks.insert_at_timestamp(BHardfork::B2, 1000);
        forks.insert_at_timestamp(AHardfork::A3, 1500);

        assert_eq!(forks.len(), 5);

        let fork_list: Vec<_> = forks.forks_iter().collect();

        // Block-based forks come first
        assert_eq!(fork_list[0].0.name(), "A1");
        assert_eq!(fork_list[0].1, ForkCondition::Block(100));
        assert_eq!(fork_list[1].0.name(), "B1");
        assert_eq!(fork_list[1].1, ForkCondition::Block(200));

        // Timestamp-based forks are ordered by timestamp
        assert_eq!(fork_list[2].0.name(), "B2");
        assert_eq!(fork_list[2].1, ForkCondition::Timestamp(1000));
        assert_eq!(fork_list[3].0.name(), "A3");
        assert_eq!(fork_list[3].1, ForkCondition::Timestamp(1500));
        assert_eq!(fork_list[4].0.name(), "A2");
        assert_eq!(fork_list[4].1, ForkCondition::Timestamp(2000));

        // Test updating a timestamp fork
        forks.insert_at_timestamp(AHardfork::A3, 500);
        assert_eq!(forks.len(), 5);

        let fork_list: Vec<_> = forks.forks_iter().collect();

        // A3 should now be first among timestamp forks
        assert_eq!(fork_list[2].0.name(), "A3");
        assert_eq!(fork_list[2].1, ForkCondition::Timestamp(500));
        assert_eq!(fork_list[3].0.name(), "B2");
        assert_eq!(fork_list[3].1, ForkCondition::Timestamp(1000));
        assert_eq!(fork_list[4].0.name(), "A2");
        assert_eq!(fork_list[4].1, ForkCondition::Timestamp(2000));
    }
}
