//! Mantle network hardfork definitions and utilities.
//!
//! This crate provides hardfork definitions specific to the Mantle network,
//! which is built on top of the OP Stack. It includes both standard OP Stack
//! hardforks and Mantle-specific upgrades like Skadi.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(not(feature = "std"))]
extern crate alloc;

// Debug utilities (requires std)
#[cfg(feature = "std")]
pub mod debug;

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;
#[cfg(feature = "serde")]
use serde as _;
use alloy_chains::{Chain, NamedChain};
use alloy_hardforks::{hardfork, EthereumHardfork};
pub use alloy_hardforks::{EthereumHardforks, ForkCondition};
use core::ops::Index;
use reth_optimism_forks::{OpHardfork, OpHardforks};
#[cfg(feature = "std")]
use std::vec::Vec;

/// Mantle mainnet chain ID
pub const MANTLE_MAINNET_CHAIN_ID: u64 = 5000;

/// Mantle Sepolia testnet chain ID  
pub const MANTLE_SEPOLIA_CHAIN_ID: u64 = 5003;

// Mantle specific hardfork timestamps
/// Skadi upgrade timestamp for Mantle mainnet
pub const MANTLE_MAINNET_SKADI_TIMESTAMP: u64 = 1_756_278_000; // Wed Aug 27 2025 15:00:00 GMT+0800

/// Skadi upgrade timestamp for Mantle Sepolia testnet
pub const MANTLE_SEPOLIA_SKADI_TIMESTAMP: u64 = 1_752_649_200; // Wed Jul 16 2025 15:00:00 GMT+0800

/// Limb upgrade timestamp for Mantle Sepolia testnet
pub const MANTLE_SEPOLIA_LIMB_TIMESTAMP: u64 = 1_764_745_200; // Wed Dec 03 2025 15:00:00 GMT+0800

/// Limb upgrade timestamp for Mantle mainnet
pub const MANTLE_MAINNET_LIMB_TIMESTAMP: u64 = 1_768_374_000; // Wed Jan 14 2026 15:00:00 GMT+0800

hardfork!(
    /// Mantle-specific hardforks that extend the OP Stack hardfork set.
    ///
    /// These hardforks represent upgrades specific to the Mantle network,
    /// building upon the foundation of OP Stack hardforks.
    #[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
    #[derive(Default)]
    MantleHardfork {
        /// Skadi: Mantle's Prague-equivalent upgrade
        ///
        /// This hardfork introduces Mantle-specific features and optimizations
        /// while maintaining compatibility with the OP Stack ecosystem.
        #[default]
        Skadi,

        /// Limb: Mantle's Osaka-equivalent upgrade
        ///
        /// This hardfork introduces Mantle-specific features and optimizations
        /// while maintaining compatibility with the OP Stack ecosystem.
        Limb,

        /// Arsia: Mantle's Arsia-equivalent upgrade
        ///
        /// This hardfork introduces Mantle-specific features and optimizations
        /// while maintaining compatibility with the OP Stack ecosystem.
        Arsia,
    }
);

impl MantleHardfork {
    /// Reverse lookup to find the hardfork given a chain ID and block timestamp.
    /// Returns the active Mantle hardfork at the given timestamp for the specified chain.
    pub fn from_chain_and_timestamp(chain: Chain, timestamp: u64) -> Option<Self> {
        let named = chain.named()?;

        match named {
            NamedChain::Mantle => Some(match timestamp {
                t if t < MANTLE_MAINNET_SKADI_TIMESTAMP => return None,
                t if t < MANTLE_MAINNET_LIMB_TIMESTAMP => Self::Skadi,
                _ => Self::Limb,
            }),
            NamedChain::MantleSepolia => Some(match timestamp {
                t if t < MANTLE_SEPOLIA_SKADI_TIMESTAMP => return None,
                t if t < MANTLE_SEPOLIA_LIMB_TIMESTAMP => Self::Skadi,
                _ => Self::Limb,
            }),
            _ => None,
        }
    }

    /// Mantle mainnet list of hardforks.
    pub const fn mantle_mainnet() -> [(Self, ForkCondition); 1] {
        [(Self::Skadi, ForkCondition::Timestamp(MANTLE_MAINNET_SKADI_TIMESTAMP))]
    }

    /// Mantle Sepolia list of hardforks.
    pub const fn mantle_sepolia() -> [(Self, ForkCondition); 2] {
        [
            (Self::Skadi, ForkCondition::Timestamp(MANTLE_SEPOLIA_SKADI_TIMESTAMP)),
            (Self::Limb, ForkCondition::Timestamp(MANTLE_SEPOLIA_LIMB_TIMESTAMP)),
        ]
    }

    /// Returns index of `self` in sorted canonical array.
    pub const fn idx(&self) -> usize {
        *self as usize
    }
}

/// Extends [`OpHardforks`] with Mantle-specific helper methods.
#[auto_impl::auto_impl(&, Arc)]
pub trait MantleHardforks: OpHardforks {
    /// Retrieves [`ForkCondition`] by a [`MantleHardfork`]. If `fork` is not present, returns
    /// [`ForkCondition::Never`].
    fn mantle_fork_activation(&self, fork: MantleHardfork) -> ForkCondition;

    /// Returns `true` if [`Skadi`](MantleHardfork::Skadi) is active at given block timestamp.
    fn is_skadi_active_at_timestamp(&self, timestamp: u64) -> bool {
        self.mantle_fork_activation(MantleHardfork::Skadi).active_at_timestamp(timestamp)
    }

    /// Returns `true` if [`Limb`](MantleHardfork::Limb) is active at given block timestamp.
    fn is_limb_active_at_timestamp(&self, timestamp: u64) -> bool {
        self.mantle_fork_activation(MantleHardfork::Limb).active_at_timestamp(timestamp)
    }

    /// Returns `true` if [`Arsia`](MantleHardfork::Arsia) is active at given block timestamp.
    fn is_arsia_active_at_timestamp(&self, timestamp: u64) -> bool {
        self.mantle_fork_activation(MantleHardfork::Arsia).active_at_timestamp(timestamp)
    }

    /// Returns the revm spec ID for Mantle chains at the given timestamp.
    ///
    /// This checks Mantle-specific hardforks (like Skadi) first, then falls back
    /// to standard OP Stack hardfork detection.
    ///
    /// # Note
    ///
    /// This is only intended to be used after Bedrock, when hardforks are activated by timestamp.
    fn revm_spec_at_timestamp(&self, timestamp: u64) -> op_revm::OpSpecId {
        // Check Mantle Arsia first
        if self.is_arsia_active_at_timestamp(timestamp) {
            op_revm::OpSpecId::ARSIA
        } else if self.is_limb_active_at_timestamp(timestamp) {
            op_revm::OpSpecId::OSAKA
        } else if self.is_skadi_active_at_timestamp(timestamp) {
            op_revm::OpSpecId::ISTHMUS
        } else {
            // Fall back to OP Stack hardforks
            alloy_op_evm::spec_by_timestamp_after_bedrock(self, timestamp)
        }
    }
}

/// A type allowing to configure activation [`ForkCondition`]s for a given list of
/// [`MantleHardfork`]s.
///
/// This extends the OP Stack hardfork system with Mantle-specific upgrades.
/// Mantle hardforks build upon the OP Stack foundation and can introduce
/// network-specific features and optimizations.
#[derive(Debug, Clone)]
pub struct MantleChainHardforks {
    /// Ordered list of Mantle hardfork activations.
    forks: Vec<(MantleHardfork, ForkCondition)>,
}

impl MantleChainHardforks {
    /// Creates a new [`MantleChainHardforks`] with the given list of forks. The input list is sorted
    /// w.r.t. the hardcoded canonicity of [`MantleHardfork`]s.
    pub fn new(forks: impl IntoIterator<Item = (MantleHardfork, ForkCondition)>) -> Self {
        let mut forks = forks.into_iter().collect::<Vec<_>>();
        forks.sort();
        Self { forks }
    }

    /// Creates a new [`MantleChainHardforks`] with Mantle mainnet configuration.
    pub fn mantle_mainnet() -> Self {
        Self::new(MantleHardfork::mantle_mainnet())
    }

    /// Creates a new [`MantleChainHardforks`] with Mantle Sepolia configuration.
    pub fn mantle_sepolia() -> Self {
        Self::new(MantleHardfork::mantle_sepolia())
    }

    /// Returns `true` if this is a Mantle mainnet instance.
    pub fn is_mantle_mainnet(&self) -> bool {
        self[MantleHardfork::Skadi] == ForkCondition::Timestamp(MANTLE_MAINNET_SKADI_TIMESTAMP)
    }
}

impl EthereumHardforks for MantleChainHardforks {
    fn ethereum_fork_activation(&self, fork: EthereumHardfork) -> ForkCondition {
        use EthereumHardfork::{Cancun, Osaka, Prague, Shanghai};
        use MantleHardfork::{Skadi, Arsia};

        if self.forks.is_empty() {
            return ForkCondition::Never;
        }

        let forks_len = self.forks.len();
        // check index out of bounds
        match fork {
            Shanghai | Cancun | Prague if forks_len <= Skadi.idx() => ForkCondition::Never,
            Osaka if forks_len <= Arsia.idx() => ForkCondition::Never,
            _ => self[fork],
        }
    }
}

impl OpHardforks for MantleChainHardforks {
    #[doc = " Retrieves [`ForkCondition`] by an [`OpHardfork`]. If `fork` is not present, returns"]
    #[doc = " [`ForkCondition::Never`]."]
    fn op_fork_activation(&self, fork: OpHardfork) -> ForkCondition {
        use reth_optimism_forks::OpHardfork;
        match fork {
            OpHardfork::Bedrock => ForkCondition::Block(0),
            OpHardfork::Regolith => ForkCondition::Timestamp(0),
            OpHardfork::Canyon
            | OpHardfork::Ecotone
            | OpHardfork::Fjord
            | OpHardfork::Granite
            | OpHardfork::Holocene
            | OpHardfork::Isthmus
            | OpHardfork::Jovian => self
                .forks
                .last()
                .map(|(_, c)| c.clone())
                .unwrap_or(ForkCondition::Timestamp(0)),
            OpHardfork::Interop => ForkCondition::Timestamp(0),
            _ => ForkCondition::Timestamp(0),
        }
    }
}

impl MantleHardforks for MantleChainHardforks {
    fn mantle_fork_activation(&self, fork: MantleHardfork) -> ForkCondition {
        // check index out of bounds
        if self.forks.len() <= fork.idx() {
            return ForkCondition::Never;
        }
        self[fork]
    }
}

impl Index<EthereumHardfork> for MantleChainHardforks {
    type Output = ForkCondition;

    fn index(&self, _hf: EthereumHardfork) -> &Self::Output {
        todo!()
    }
}

impl Index<OpHardfork> for MantleChainHardforks {
    type Output = ForkCondition;

    fn index(&self, _hf: OpHardfork) -> &Self::Output {
        todo!()
    }
}

impl Index<MantleHardfork> for MantleChainHardforks {
    type Output = ForkCondition;

    fn index(&self, hf: MantleHardfork) -> &Self::Output {
        use MantleHardfork::{Arsia, Limb, Skadi};

        match hf {
            Skadi => &self.forks[Skadi.idx()].1,
            Limb => &self.forks[Limb.idx()].1,
            Arsia => &self.forks[Arsia.idx()].1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::str::FromStr;

    #[test]
    fn check_mantle_hardfork_from_str() {
        let hardfork_str = ["skadi", "Skadi", "SKADI"];
        let expected_hardfork = MantleHardfork::Skadi;

        for s in hardfork_str {
            let hardfork = MantleHardfork::from_str(s).unwrap();
            assert_eq!(hardfork, expected_hardfork);
        }
    }

    #[test]
    fn check_nonexistent_hardfork_from_str() {
        assert!(MantleHardfork::from_str("not a hardfork").is_err());
    }

    #[test]
    fn mantle_mainnet_fork_conditions() {
        use MantleHardfork::*;

        let mantle_mainnet_forks = MantleChainHardforks::mantle_mainnet();
        assert_eq!(
            mantle_mainnet_forks[Skadi],
            ForkCondition::Timestamp(MANTLE_MAINNET_SKADI_TIMESTAMP)
        );
    }

    #[test]
    fn mantle_sepolia_fork_conditions() {
        use MantleHardfork::*;

        let mantle_sepolia_forks = MantleChainHardforks::mantle_sepolia();
        assert_eq!(
            mantle_sepolia_forks[Skadi],
            ForkCondition::Timestamp(MANTLE_SEPOLIA_SKADI_TIMESTAMP)
        );
    }

    #[test]
    fn test_reverse_lookup_mantle_chains() {
        // Test Mantle mainnet
        let mantle_mainnet_chain = Chain::from_id(MANTLE_MAINNET_CHAIN_ID);
        assert_eq!(
            MantleHardfork::from_chain_and_timestamp(
                mantle_mainnet_chain,
                MANTLE_MAINNET_SKADI_TIMESTAMP
            ),
            Some(MantleHardfork::Skadi)
        );
        assert_eq!(
            MantleHardfork::from_chain_and_timestamp(
                mantle_mainnet_chain,
                MANTLE_MAINNET_SKADI_TIMESTAMP - 1
            ),
            None
        );

        // Test Mantle Sepolia
        let mantle_sepolia_chain = Chain::from_id(MANTLE_SEPOLIA_CHAIN_ID);
        assert_eq!(
            MantleHardfork::from_chain_and_timestamp(
                mantle_sepolia_chain,
                MANTLE_SEPOLIA_SKADI_TIMESTAMP
            ),
            Some(MantleHardfork::Skadi)
        );
        assert_eq!(
            MantleHardfork::from_chain_and_timestamp(
                mantle_sepolia_chain,
                MANTLE_SEPOLIA_SKADI_TIMESTAMP - 1
            ),
            None
        );

        // Test non-Mantle chain
        assert_eq!(MantleHardfork::from_chain_and_timestamp(Chain::from_id(999999), 1000000), None);
    }

    #[test]
    fn test_reverse_lookup_limb_hardfork() {
        let mantle_sepolia_chain = Chain::from_id(MANTLE_SEPOLIA_CHAIN_ID);

        // Test Limb activation
        assert_eq!(
            MantleHardfork::from_chain_and_timestamp(
                mantle_sepolia_chain,
                MANTLE_SEPOLIA_LIMB_TIMESTAMP
            ),
            Some(MantleHardfork::Limb)
        );

        // Test before Limb but after Skadi
        assert_eq!(
            MantleHardfork::from_chain_and_timestamp(
                mantle_sepolia_chain,
                MANTLE_SEPOLIA_LIMB_TIMESTAMP - 1
            ),
            Some(MantleHardfork::Skadi)
        );
    }

    #[test]
    fn test_limb_fork_condition() {
        let mantle_sepolia_forks = MantleChainHardforks::mantle_sepolia();
        assert_eq!(
            mantle_sepolia_forks[MantleHardfork::Limb],
            ForkCondition::Timestamp(MANTLE_SEPOLIA_LIMB_TIMESTAMP)
        );
    }
}
