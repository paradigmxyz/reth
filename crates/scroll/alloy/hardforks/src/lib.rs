//! Scroll-Reth hard forks.

#![cfg_attr(not(feature = "std"), no_std)]
#[cfg(not(feature = "std"))]
extern crate alloc as std;

use alloy_hardforks::{EthereumHardfork, EthereumHardforks, ForkCondition};
use std::vec::Vec;

pub use hardfork::ScrollHardfork;
pub mod hardfork;

/// Extends [`EthereumHardforks`] with scroll helper methods.
#[auto_impl::auto_impl(&, Arc)]
pub trait ScrollHardforks: EthereumHardforks {
    /// Retrieves [`ForkCondition`] by an [`ScrollHardfork`]. If `fork` is not present, returns
    /// [`ForkCondition::Never`].
    fn scroll_fork_activation(&self, fork: ScrollHardfork) -> ForkCondition;

    /// Convenience method to check if [`Bernoulli`](ScrollHardfork::Bernoulli) is active at a given
    /// block number.
    fn is_bernoulli_active_at_block(&self, block_number: u64) -> bool {
        self.scroll_fork_activation(ScrollHardfork::Bernoulli).active_at_block(block_number)
    }

    /// Returns `true` if [`Curie`](ScrollHardfork::Curie) is active at given block block number.
    fn is_curie_active_at_block(&self, block_number: u64) -> bool {
        self.scroll_fork_activation(ScrollHardfork::Curie).active_at_block(block_number)
    }

    /// Returns `true` if [`Darwin`](ScrollHardfork::Darwin) is active at given block timestamp.
    fn is_darwin_active_at_timestamp(&self, timestamp: u64) -> bool {
        self.scroll_fork_activation(ScrollHardfork::Darwin).active_at_timestamp(timestamp)
    }

    /// Returns `true` if [`DarwinV2`](ScrollHardfork::DarwinV2) is active at given block timestamp.
    fn is_darwin_v2_active_at_timestamp(&self, timestamp: u64) -> bool {
        self.scroll_fork_activation(ScrollHardfork::DarwinV2).active_at_timestamp(timestamp)
    }

    /// Returns `true` if [`Euclid`](ScrollHardfork::Euclid) is active at given block timestamp.
    fn is_euclid_active_at_timestamp(&self, timestamp: u64) -> bool {
        self.scroll_fork_activation(ScrollHardfork::Euclid).active_at_timestamp(timestamp)
    }

    /// Returns `true` if [`EuclidV2`](ScrollHardfork::EuclidV2) is active at given block timestamp.
    fn is_euclid_v2_active_at_timestamp(&self, timestamp: u64) -> bool {
        self.scroll_fork_activation(ScrollHardfork::EuclidV2).active_at_timestamp(timestamp)
    }

    /// Returns `true` if [`Feynman`](ScrollHardfork::Feynman) is active at given block timestamp.
    fn is_feynman_active_at_timestamp(&self, timestamp: u64) -> bool {
        self.scroll_fork_activation(ScrollHardfork::Feynman).active_at_timestamp(timestamp)
    }
}

/// A type allowing to configure activation [`ForkCondition`]s for a given list of
/// [`ScrollHardfork`]s.
#[derive(Debug, Clone)]
pub struct ScrollChainHardforks {
    /// Scroll hardfork activations.
    forks: Vec<(ScrollHardfork, ForkCondition)>,
}

impl ScrollChainHardforks {
    /// Creates a new [`ScrollChainHardforks`] with the given list of forks.
    pub fn new(forks: impl IntoIterator<Item = (ScrollHardfork, ForkCondition)>) -> Self {
        let mut forks = forks.into_iter().collect::<Vec<_>>();
        forks.sort();
        Self { forks }
    }

    /// Creates a new [`ScrollChainHardforks`] with Scroll mainnet configuration.
    pub fn scroll_mainnet() -> Self {
        Self::new(ScrollHardfork::scroll_mainnet())
    }

    /// Creates a new [`ScrollChainHardforks`] with Scroll Sepolia configuration.
    pub fn scroll_sepolia() -> Self {
        Self::new(ScrollHardfork::scroll_sepolia())
    }
}

impl EthereumHardforks for ScrollChainHardforks {
    fn ethereum_fork_activation(&self, fork: EthereumHardfork) -> ForkCondition {
        if fork < EthereumHardfork::ArrowGlacier {
            ForkCondition::Block(0)
        } else if fork <= EthereumHardfork::Shanghai {
            self.scroll_fork_activation(ScrollHardfork::Bernoulli)
        } else {
            ForkCondition::Never
        }
    }
}

impl ScrollHardforks for ScrollChainHardforks {
    fn scroll_fork_activation(&self, fork: ScrollHardfork) -> ForkCondition {
        let Ok(idx) = self.forks.binary_search_by(|(f, _)| f.cmp(&fork)) else {
            return ForkCondition::Never;
        };

        self.forks[idx].1
    }
}
