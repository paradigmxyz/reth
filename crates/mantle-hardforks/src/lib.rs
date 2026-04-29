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
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(not(feature = "std"))]
extern crate alloc;

// Debug utilities (requires std)
#[cfg(feature = "std")]
pub mod debug;

#[cfg(not(feature = "std"))]
use alloc::{vec, vec::Vec};
use alloy_chains::{Chain, NamedChain};
use alloy_hardforks::{hardfork, EthereumHardfork};
pub use alloy_hardforks::{EthereumHardforks, ForkCondition};
#[cfg(feature = "state-export")]
use alloy_primitives as _;
use core::ops::Index;
#[cfg(feature = "state-export")]
use eyre as _;
#[cfg(feature = "state-export")]
use reth_db as _;
#[cfg(feature = "state-export")]
use reth_db_api as _;
use reth_optimism_forks::{OpHardfork, OpHardforks};
#[cfg(feature = "state-export")]
use reth_primitives_traits as _;
#[cfg(feature = "state-export")]
use reth_provider as _;
#[cfg(feature = "state-export")]
use reth_trie as _;
#[cfg(feature = "state-export")]
use reth_trie_db as _;
#[cfg(feature = "state-export")]
use revm as _;
#[cfg(any(feature = "serde", feature = "state-export"))]
use serde as _;
#[cfg(feature = "std")]
use std::vec::Vec;
#[cfg(feature = "state-export")]
use tracing as _;

/// Mantle mainnet chain ID
pub const MANTLE_MAINNET_CHAIN_ID: u64 = 5000;

/// Mantle Sepolia testnet chain ID
pub const MANTLE_SEPOLIA_CHAIN_ID: u64 = 5003;

/// 32-byte prefix that identifies a Mantle `MetaTx`.
///
/// The prefix is the ASCII string `"MantleMetaTxPrefix"` (18 bytes) right-aligned
/// in a 32-byte field (14 leading zero bytes).
///
/// `MetaTx` was a Mantle-specific gas sponsorship mechanism where a relayer wraps an
/// inner transaction with a sponsor signature. The outer EIP-1559 transaction carries
/// this prefix in its `input` field, followed by the RLP-encoded inner transaction.
///
/// **`MetaTx` has been permanently disabled since the `MantleEverest` hardfork
/// (mainnet: 2025-03-19, sepolia: 2025-01-16).** All Mantle chains now reject
/// transactions whose input starts with this prefix.
///
/// Reference: `op-geth/core/types/meta_transaction.go` — `MetaTxPrefix`
pub const MANTLE_META_TX_PREFIX: [u8; 32] = [
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // 8 zero bytes
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // 6 zero bytes
    0x4D, 0x61, 0x6E, 0x74, 0x6C, 0x65, // "Mantle"
    0x4D, 0x65, 0x74, 0x61, 0x54, 0x78, // "MetaTx"
    0x50, 0x72, 0x65, 0x66, 0x69, 0x78, // "Prefix"
];

/// Returns `true` if `input` starts with the [`MANTLE_META_TX_PREFIX`].
///
/// `MetaTx` transactions are legacy Mantle gas-sponsorship transactions that have been
/// permanently disabled since the `MantleEverest` hardfork. Any transaction whose
/// calldata begins with this 32-byte prefix must be rejected.
#[inline]
pub fn is_mantle_meta_tx(input: &[u8]) -> bool {
    input.len() > 32 && input[..32] == MANTLE_META_TX_PREFIX
}

// Mantle specific hardfork timestamps
/// Skadi upgrade timestamp for Mantle mainnet
pub const MANTLE_MAINNET_SKADI_TIMESTAMP: u64 = 1_756_278_000; // Wed Aug 27 2025 15:00:00 GMT+0800

/// Limb upgrade timestamp for Mantle mainnet
pub const MANTLE_MAINNET_LIMB_TIMESTAMP: u64 = 1_768_374_000; // Wed Jan 14 2026 15:00:00 GMT+0800

/// Arsia upgrade timestamp for Mantle mainnet
pub const MANTLE_MAINNET_ARSIA_TIMESTAMP: u64 = 1_776_841_200; // Wed Apr 22 2026 15:00:00 GMT+0800

/// Skadi upgrade timestamp for Mantle Sepolia testnet
pub const MANTLE_SEPOLIA_SKADI_TIMESTAMP: u64 = 1_752_649_200; // Wed Jul 16 2025 15:00:00 GMT+0800

/// Limb upgrade timestamp for Mantle Sepolia testnet
pub const MANTLE_SEPOLIA_LIMB_TIMESTAMP: u64 = 1_764_745_200; // Wed Dec 03 2025 15:00:00 GMT+0800

/// Arsia upgrade timestamp for Mantle Sepolia testnet
pub const MANTLE_SEPOLIA_ARSIA_TIMESTAMP: u64 = 1_774_422_000; // Wed Mar 25 2026 15:00:00 GMT+0800

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
            NamedChain::Mantle => {
                if timestamp < MANTLE_MAINNET_SKADI_TIMESTAMP {
                    return None;
                }
                if timestamp < MANTLE_MAINNET_LIMB_TIMESTAMP {
                    return Some(Self::Skadi);
                }
                if timestamp < MANTLE_MAINNET_ARSIA_TIMESTAMP {
                    return Some(Self::Limb);
                }
                Some(Self::Arsia)
            }
            NamedChain::MantleSepolia => {
                if timestamp < MANTLE_SEPOLIA_SKADI_TIMESTAMP {
                    return None;
                }
                if timestamp < MANTLE_SEPOLIA_LIMB_TIMESTAMP {
                    return Some(Self::Skadi);
                }
                if timestamp < MANTLE_SEPOLIA_ARSIA_TIMESTAMP {
                    return Some(Self::Limb);
                }
                Some(Self::Arsia)
            }
            _ => None,
        }
    }

    /// Mantle mainnet list of hardforks.
    pub fn mantle_mainnet() -> Vec<(Self, ForkCondition)> {
        vec![
            (Self::Skadi, ForkCondition::Timestamp(MANTLE_MAINNET_SKADI_TIMESTAMP)),
            (Self::Limb, ForkCondition::Timestamp(MANTLE_MAINNET_LIMB_TIMESTAMP)),
            (Self::Arsia, ForkCondition::Timestamp(MANTLE_MAINNET_ARSIA_TIMESTAMP)),
        ]
    }

    /// Mantle Sepolia list of hardforks.
    pub fn mantle_sepolia() -> Vec<(Self, ForkCondition)> {
        vec![
            (Self::Skadi, ForkCondition::Timestamp(MANTLE_SEPOLIA_SKADI_TIMESTAMP)),
            (Self::Limb, ForkCondition::Timestamp(MANTLE_SEPOLIA_LIMB_TIMESTAMP)),
            (Self::Arsia, ForkCondition::Timestamp(MANTLE_SEPOLIA_ARSIA_TIMESTAMP)),
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

    /// Returns `true` if this chain uses Mantle semantics.
    /// Used e.g. for receipt root calculation where Mantle excludes both
    /// `deposit_nonce` and `deposit_receipt_version` from the trie.
    fn is_mantle_chain(&self) -> bool;

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
    /// Creates a new [`MantleChainHardforks`] with the given list of forks. The input list is
    /// sorted w.r.t. the hardcoded canonicity of [`MantleHardfork`]s.
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
        use MantleHardfork::{Arsia, Skadi};

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
            OpHardfork::Canyon |
            OpHardfork::Ecotone |
            OpHardfork::Fjord |
            OpHardfork::Granite |
            OpHardfork::Holocene |
            OpHardfork::Isthmus |
            OpHardfork::Jovian => {
                self.forks.last().map(|(_, c)| *c).unwrap_or(ForkCondition::Timestamp(0))
            }
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

    fn is_mantle_chain(&self) -> bool {
        true
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
        assert_eq!(
            mantle_mainnet_forks[Limb],
            ForkCondition::Timestamp(MANTLE_MAINNET_LIMB_TIMESTAMP)
        );
        assert_eq!(
            mantle_mainnet_forks[Arsia],
            ForkCondition::Timestamp(MANTLE_MAINNET_ARSIA_TIMESTAMP)
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
        assert_eq!(
            mantle_sepolia_forks[Limb],
            ForkCondition::Timestamp(MANTLE_SEPOLIA_LIMB_TIMESTAMP)
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
    fn test_reverse_lookup_mainnet_limb_hardfork() {
        let mantle_mainnet_chain = Chain::from_id(MANTLE_MAINNET_CHAIN_ID);

        assert_eq!(
            MantleHardfork::from_chain_and_timestamp(
                mantle_mainnet_chain,
                MANTLE_MAINNET_LIMB_TIMESTAMP
            ),
            Some(MantleHardfork::Limb)
        );

        assert_eq!(
            MantleHardfork::from_chain_and_timestamp(
                mantle_mainnet_chain,
                MANTLE_MAINNET_LIMB_TIMESTAMP - 1
            ),
            Some(MantleHardfork::Skadi)
        );
    }

    #[test]
    fn test_reverse_lookup_mainnet_arsia_hardfork() {
        let mantle_mainnet_chain = Chain::from_id(MANTLE_MAINNET_CHAIN_ID);

        // Arsia should be active at its timestamp
        assert_eq!(
            MantleHardfork::from_chain_and_timestamp(
                mantle_mainnet_chain,
                MANTLE_MAINNET_ARSIA_TIMESTAMP
            ),
            Some(MantleHardfork::Arsia)
        );

        // One second before Arsia: still Limb
        assert_eq!(
            MantleHardfork::from_chain_and_timestamp(
                mantle_mainnet_chain,
                MANTLE_MAINNET_ARSIA_TIMESTAMP - 1
            ),
            Some(MantleHardfork::Limb)
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

    // ---- MetaTx prefix tests ----

    #[test]
    fn meta_tx_prefix_matches_ascii() {
        // Guard against typos: the trailing 18 bytes must decode to "MantleMetaTxPrefix"
        assert_eq!(&MANTLE_META_TX_PREFIX[14..], b"MantleMetaTxPrefix");
    }

    #[test]
    fn is_meta_tx_with_valid_prefix_and_payload() {
        let mut input = MANTLE_META_TX_PREFIX.to_vec();
        input.push(0xF8); // minimal RLP stub
        assert!(is_mantle_meta_tx(&input));
    }

    #[test]
    fn is_meta_tx_exactly_32_bytes_not_flagged() {
        // op-geth: len(txData) <= MetaTxPrefixLength → return nil
        assert!(!is_mantle_meta_tx(&MANTLE_META_TX_PREFIX));
    }

    #[test]
    fn is_meta_tx_empty_and_short_input() {
        assert!(!is_mantle_meta_tx(&[]));
        assert!(!is_mantle_meta_tx(&[0u8; 10]));
        assert!(!is_mantle_meta_tx(&[0u8; 32]));
    }

    #[test]
    fn is_meta_tx_wrong_prefix() {
        let mut input = vec![0u8; 33];
        input[14] = 0xFF; // corrupt one byte of the prefix
        assert!(!is_mantle_meta_tx(&input));
    }

    #[test]
    fn is_meta_tx_all_zeros_not_flagged() {
        // 33 zero bytes is long enough but lacks the ASCII suffix → not a MetaTx
        assert!(!is_mantle_meta_tx(&[0u8; 33]));
    }
}
