//! OP-Reth hard forks.
//!
//! This defines the [`ChainHardforks`] for certain op chains.
//! It keeps L2 hardforks that correspond to L1 hardforks in sync by defining both at the same
//! activation timestamp, this includes:
//!  - Canyon : Shanghai
//!  - Ecotone : Cancun
//!  - Isthmus : Prague

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]

extern crate alloc;

// Re-export alloy-op-hardforks types.
use alloy_op_hardforks::{EthereumHardforks, OpChainHardforks};
pub use alloy_op_hardforks::{OpHardfork, OpHardforks};

use alloc::vec;
use alloy_primitives::U256;
use once_cell::sync::Lazy as LazyLock;
use reth_ethereum_forks::{ChainHardforks, EthereumHardfork, ForkCondition, Hardfork};

/// Dev hardforks
pub static DEV_HARDFORKS: LazyLock<ChainHardforks> = LazyLock::new(|| {
    ChainHardforks::new(vec![
        (EthereumHardfork::Frontier.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Homestead.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Dao.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Tangerine.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::SpuriousDragon.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Byzantium.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Constantinople.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Petersburg.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Istanbul.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Berlin.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::London.boxed(), ForkCondition::Block(0)),
        (
            EthereumHardfork::Paris.boxed(),
            ForkCondition::TTD {
                activation_block_number: 0,
                fork_block: None,
                total_difficulty: U256::ZERO,
            },
        ),
        (OpHardfork::Bedrock.boxed(), ForkCondition::Block(0)),
        (OpHardfork::Regolith.boxed(), ForkCondition::Timestamp(0)),
        (EthereumHardfork::Shanghai.boxed(), ForkCondition::Timestamp(0)),
        (OpHardfork::Canyon.boxed(), ForkCondition::Timestamp(0)),
        (EthereumHardfork::Cancun.boxed(), ForkCondition::Timestamp(0)),
        (OpHardfork::Ecotone.boxed(), ForkCondition::Timestamp(0)),
        (OpHardfork::Fjord.boxed(), ForkCondition::Timestamp(0)),
        (OpHardfork::Granite.boxed(), ForkCondition::Timestamp(0)),
        (EthereumHardfork::Prague.boxed(), ForkCondition::Timestamp(0)),
        (OpHardfork::Isthmus.boxed(), ForkCondition::Timestamp(0)),
        // (OpHardfork::Jovian.boxed(), ForkCondition::Timestamp(0)),
    ])
});

/// Helper function to initialize Reth's `ChainHardforks` from Alloy's `OpChainHardforks` init
/// functions.
pub fn chain_hardforks(op_hardforks: OpChainHardforks) -> ChainHardforks {
    let mut forks = Vec::new();
    EthereumHardfork::VARIANTS.iter().for_each(|ethereum_hardfork| {
        forks.push((
            ethereum_hardfork.boxed(),
            op_hardforks.ethereum_fork_activation(*ethereum_hardfork),
        ));
    });
    OpHardfork::VARIANTS.iter().for_each(|op_hardfork| {
        forks.push((op_hardfork.boxed(), op_hardforks.op_fork_activation(*op_hardfork)));
    });
    ChainHardforks::new(forks)
}

/// Optimism mainnet list of hardforks.
pub static OP_MAINNET_HARDFORKS: LazyLock<ChainHardforks> = LazyLock::new(|| {
    // Build from alloy-op-hardforks canonical OP mapping to avoid magic numbers.
    chain_hardforks(OpChainHardforks::op_mainnet())
});
/// Optimism Sepolia list of hardforks.
pub static OP_SEPOLIA_HARDFORKS: LazyLock<ChainHardforks> =
    LazyLock::new(|| chain_hardforks(OpChainHardforks::op_sepolia()));

/// Base Sepolia list of hardforks.
pub static BASE_SEPOLIA_HARDFORKS: LazyLock<ChainHardforks> =
    LazyLock::new(|| chain_hardforks(OpChainHardforks::base_sepolia()));

/// Base mainnet list of hardforks.
pub static BASE_MAINNET_HARDFORKS: LazyLock<ChainHardforks> =
    LazyLock::new(|| chain_hardforks(OpChainHardforks::base_mainnet()));
