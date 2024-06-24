use core::ops::Deref;

use crate::{ForkCondition, Hardfork};

/// Generic trait over a set of ordered hardforks
pub trait Hardforks: Default + Clone {
    /// Retrieves [`ForkCondition`] from `fork`. If `fork` is not present, returns
    /// [`ForkCondition::Never`].
    fn fork<H: Hardfork>(&self, fork: H) -> ForkCondition {
        self.get(fork).unwrap_or(ForkCondition::Never)
    }

    /// Retrieves [`ForkCondition`] from `fork` if it exists, otherwise `None`.
    fn get<H: Hardfork>(&self, fork: H) -> Option<ForkCondition>;

    /// Removes `fork` from list.
    fn remove<H: Hardfork>(&mut self, fork: H);

    /// Inserts `fork` into list.
    fn insert<H: Hardfork>(&mut self, fork: H, condition: ForkCondition);

    /// Get an iterator of all hardforks with their respective activation conditions.
    fn forks_iter(
        &self,
    ) -> impl Iterator<Item = (&Box<dyn Hardfork + 'static>, ForkCondition)>;

    /// Convenience method to check if a fork is active at a given timestamp.
    fn is_fork_active_at_timestamp<H: Hardfork>(&self, fork: H, timestamp: u64) -> bool {
        self.fork(fork).active_at_timestamp(timestamp)
    }

    /// Convenience method to check if a fork is active at a given block number.
    fn is_fork_active_at_block<H: Hardfork>(&self, fork: H, block_number: u64) -> bool {
        self.fork(fork).active_at_block(block_number)
    }
}

/// Base type over a list of hardforks that implement `HardforkTrait`.
#[derive(Default, Clone, PartialEq, Eq)]
pub struct ChainHardforks(pub Vec<(Box<dyn Hardfork>, ForkCondition)>);

impl Deref for ChainHardforks {
    type Target = Vec<(Box<dyn Hardfork>, ForkCondition)>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Hardforks for ChainHardforks {
    fn get<H: Hardfork>(&self, fork: H) -> Option<ForkCondition> {
        for (inner_fork, condition) in &self.0 {
            if fork.name() == inner_fork.name() {
                return Some(*condition)
            }
        }
        None
    }

    fn forks_iter(
        &self,
    ) -> impl Iterator<Item = (&Box<dyn Hardfork + 'static>, ForkCondition)> {
        self.0.iter().map(|(f, b)| (f, *b))
    }

    fn insert<H: Hardfork>(&mut self, fork: H, condition: ForkCondition) {
        self.0.push((Box::new(fork), condition));
    }

    fn remove<H: Hardfork>(&mut self, fork: H) {
        self.0.retain(|(inner_fork, _)| inner_fork.name() != fork.name());
    }
}

impl core::fmt::Debug for ChainHardforks {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("HardforksBaseType")
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
