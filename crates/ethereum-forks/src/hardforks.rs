use core::ops::Deref;

use crate::{ForkCondition, Hardfork};

/// Generic trait over a set of ordered hardforks
pub trait Hardforks: Default + Clone {
    /// Retrieves [`ForkCondition`] from `fork`. If `fork` is not present, returns
    /// [`ForkCondition::Never`].
    fn fork<H: Hardfork>(&self, fork: H) -> ForkCondition;

    /// Get an iterator of all hardforks with their respective activation conditions.
    fn forks_iter(&self) -> impl Iterator<Item = (&Box<dyn Hardfork + 'static>, ForkCondition)>;

    /// Convenience method to check if a fork is active at a given timestamp.
    fn is_fork_active_at_timestamp<H: Hardfork>(&self, fork: H, timestamp: u64) -> bool {
        self.fork(fork).active_at_timestamp(timestamp)
    }

    /// Convenience method to check if a fork is active at a given block number.
    fn is_fork_active_at_block<H: Hardfork>(&self, fork: H, block_number: u64) -> bool {
        self.fork(fork).active_at_block(block_number)
    }
}

/// Ordered list of a chain hardforks that implement [`Hardfork`].
#[derive(Default, Clone, PartialEq, Eq)]
pub struct ChainHardforks(pub Vec<(Box<dyn Hardfork>, ForkCondition)>);

impl Deref for ChainHardforks {
    type Target = Vec<(Box<dyn Hardfork>, ForkCondition)>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl ChainHardforks {
    /// Retrieves [`ForkCondition`] from `fork`. If `fork` is not present, returns
    /// [`ForkCondition::Never`].
    pub fn fork<H: Hardfork>(&self, fork: H) -> ForkCondition {
        self.get(fork).unwrap_or(ForkCondition::Never)
    }

    /// Retrieves [`ForkCondition`] from `fork` if it exists, otherwise `None`.
    pub fn get<H: Hardfork>(&self, fork: H) -> Option<ForkCondition> {
        for (inner_fork, condition) in &self.0 {
            if fork.name() == inner_fork.name() {
                return Some(*condition)
            }
        }
        None
    }

    /// Get an iterator of all hardforks with their respective activation conditions.
    pub fn forks_iter(
        &self,
    ) -> impl Iterator<Item = (&Box<dyn Hardfork + 'static>, ForkCondition)> {
        self.0.iter().map(|(f, b)| (f, *b))
    }

    /// Convenience method to check if a fork is active at a given timestamp.
    pub fn is_fork_active_at_timestamp<H: Hardfork>(&self, fork: H, timestamp: u64) -> bool {
        self.fork(fork).active_at_timestamp(timestamp)
    }

    /// Convenience method to check if a fork is active at a given block number.
    pub fn is_fork_active_at_block<H: Hardfork>(&self, fork: H, block_number: u64) -> bool {
        self.fork(fork).active_at_block(block_number)
    }

    /// Inserts `fork` into list.
    pub fn insert<H: Hardfork>(&mut self, fork: H, condition: ForkCondition) {
        self.0.push((Box::new(fork), condition));
    }

    /// Removes `fork` from list.
    pub fn remove<H: Hardfork>(&mut self, fork: H) {
        self.0.retain(|(inner_fork, _)| inner_fork.name() != fork.name());
    }
}

impl Hardforks for ChainHardforks {
    fn fork<H: Hardfork>(&self, fork: H) -> ForkCondition {
        self.fork(fork)
    }

    fn forks_iter(&self) -> impl Iterator<Item = (&Box<dyn Hardfork + 'static>, ForkCondition)> {
        self.forks_iter()
    }
}

impl core::fmt::Debug for ChainHardforks {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ChainHardforks")
            .field(
                "0",
                &self
                    .0
                    .iter()
                    .map(|(hf, cond)| {
                        // Create a tuple with references to implement Debug
                        (hf.name(), cond)
                    })
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}
