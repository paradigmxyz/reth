/// Ethereum helper methods
mod ethereum;
pub use ethereum::{EthereumActivations, EthereumHardforks};

/// Optimism helper methods
#[cfg(feature = "optimism")]
mod optimism;
#[cfg(feature = "optimism")]
pub use optimism::{OptimismActivations, OptimismHardforks};

use crate::{ForkCondition, Hardfork};
use std::collections::HashMap;

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
pub struct ChainHardforks {
    forks: Vec<(Box<dyn Hardfork>, ForkCondition)>,
    map: HashMap<&'static str, ForkCondition>,
}

impl ChainHardforks {
    /// Creates a new [`ChainHardforks`] from a list.
    pub fn new(forks: Vec<(Box<dyn Hardfork>, ForkCondition)>) -> Self {
        let mut index = HashMap::default();
        for (fork, condition) in &forks {
            index.insert(fork.name(), *condition);
        }

        Self { forks, map: index }
    }

    /// Total number of hardforks.
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> usize {
        self.forks.len()
    }

    /// Retrieves [`ForkCondition`] from `fork`. If `fork` is not present, returns
    /// [`ForkCondition::Never`].
    pub fn fork<H: Hardfork>(&self, fork: H) -> ForkCondition {
        self.get(fork).unwrap_or(ForkCondition::Never)
    }

    /// Retrieves [`ForkCondition`] from `fork` if it exists, otherwise `None`.
    pub fn get<H: Hardfork>(&self, fork: H) -> Option<ForkCondition> {
        self.map.get(fork.name()).copied()
    }

    /// Get an iterator of all hardforks with their respective activation conditions.
    pub fn forks_iter(
        &self,
    ) -> impl Iterator<Item = (&Box<dyn Hardfork + 'static>, ForkCondition)> {
        self.forks.iter().map(|(f, b)| (f, *b))
    }

    /// Convenience method to check if a fork is active at a given timestamp.
    pub fn is_fork_active_at_timestamp<H: Hardfork>(&self, fork: H, timestamp: u64) -> bool {
        self.fork(fork).active_at_timestamp(timestamp)
    }

    /// Convenience method to check if a fork is active at a given block number.
    pub fn is_fork_active_at_block<H: Hardfork>(&self, fork: H, block_number: u64) -> bool {
        self.fork(fork).active_at_block(block_number)
    }

    /// Inserts `fork` into list, updating with a new [`ForkCondition`] if it already exists.
    pub fn insert<H: Hardfork>(&mut self, fork: H, condition: ForkCondition) {
        match self.map.entry(fork.name()) {
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                *entry.get_mut() = condition;
                if let Some((_, inner)) =
                    self.forks.iter_mut().find(|(inner, _)| inner.name() == fork.name())
                {
                    *inner = condition;
                }
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(condition);
                self.forks.push((Box::new(fork), condition));
            }
        }
    }

    /// Removes `fork` from list.
    pub fn remove<H: Hardfork>(&mut self, fork: H) {
        self.forks.retain(|(inner_fork, _)| inner_fork.name() != fork.name());
        self.map.remove(fork.name());
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
            .field("0", &self.forks_iter().map(|(hf, cond)| (hf.name(), cond)).collect::<Vec<_>>())
            .finish()
    }
}
