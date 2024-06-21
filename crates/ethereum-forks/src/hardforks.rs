use crate::{ForkCondition, Hardfork, HardforkTrait};

/// Generic trait over a set of ordered hardforks
pub trait HardforksTrait: Default + Clone {
    /// Retrieves [`ForkCondition`] from `fork`. If `fork` is not present, returns
    /// [`ForkCondition::Never`].
    fn fork<H: HardforkTrait>(&self, fork: H) -> ForkCondition {
        self.get(fork).unwrap_or(ForkCondition::Never)
    }

    /// Retrieves [`ForkCondition`] from `fork` if it exists, otherwise `None`.
    fn get<H: HardforkTrait>(&self, fork: H) -> Option<ForkCondition>;

    /// Removes `fork` from list.
    fn remove<H: HardforkTrait>(&mut self, fork: H);

    /// Inserts `fork` into list.
    fn insert<H: HardforkTrait>(&mut self, fork: H, condition: ForkCondition);

    /// Get an iterator of all hardforks with their respective activation conditions.
    fn forks_iter(
        &self,
    ) -> impl Iterator<Item = (&Box<dyn HardforkTrait + 'static>, ForkCondition)>;

    /// Convenience method to check if a fork is active at a given timestamp.
    fn is_fork_active_at_timestamp<H: HardforkTrait>(&self, fork: H, timestamp: u64) -> bool {
        self.fork(fork).active_at_timestamp(timestamp)
    }

    /// Convenience method to check if a fork is active at a given block number.
    fn is_fork_active_at_block<H: HardforkTrait>(&self, fork: H, block_number: u64) -> bool {
        self.fork(fork).active_at_block(block_number)
    }
}

/// Base type over a list of hardforks that implement `HardforkTrait`.
pub type HardforksBaseType = Vec<(Box<dyn HardforkTrait>, ForkCondition)>;

impl HardforksTrait for HardforksBaseType {
    fn get<H: HardforkTrait>(&self, fork: H) -> Option<ForkCondition> {
        for (inner_fork, condition) in self {
            if fork.name() == inner_fork.name() {
                return Some(*condition)
            }
        }
        None
    }

    fn forks_iter(
        &self,
    ) -> impl Iterator<Item = (&Box<dyn HardforkTrait + 'static>, ForkCondition)> {
        self.iter().map(|(f, b)| (f, *b))
    }

    fn insert<H: HardforkTrait>(&mut self, fork: H, condition: ForkCondition) {
        self.push((Box::new(fork), condition));
    }

    fn remove<H: HardforkTrait>(&mut self, fork: H) {
        self.retain(|(inner_fork, _)| inner_fork.name() != fork.name());
    }
}

/// Macro to generate a wrapper type over [`HardforksBaseType`] that implements [`HardforksTrait`].
#[macro_export]
macro_rules! generate_forks_type {
    // Match with optional documentation
    ($(#[$meta:meta])* $forks:ident) => {
        $(#[$meta])*
        #[derive(Clone, Default)]
        pub struct $forks(pub HardforksBaseType);

        impl From<HardforksBaseType> for $forks {
            fn from(forks: HardforksBaseType) -> Self {
                $forks(forks)
            }
        }

        impl HardforksTrait for $forks {
            fn get<H: HardforkTrait>(&self, fork: H) -> Option<ForkCondition> {
                HardforksTrait::get(&self.0, fork)
            }

            fn forks_iter(
                &self,
            ) -> impl Iterator<Item = (&Box<dyn HardforkTrait + 'static>, ForkCondition)> {
                HardforksTrait::forks_iter(&self.0)
            }

            fn insert<H: HardforkTrait>(&mut self, fork: H, condition: ForkCondition) {
                HardforksTrait::insert(&mut self.0, fork, condition);
            }

            fn remove<H: HardforkTrait>(&mut self, fork: H) {
                HardforksTrait::remove(&mut self.0, fork);
            }
        }

        impl core::fmt::Debug for $forks {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                f.debug_struct(stringify!($forks))
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
    };
}
