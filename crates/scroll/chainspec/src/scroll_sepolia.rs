//! Chain specification for the Scroll Sepolia testnet network.

use alloc::sync::Arc;

use alloy_chains::{Chain, NamedChain};
use alloy_primitives::b256;
use reth_chainspec::{once_cell_set, ChainSpec};
use reth_scroll_forks::ScrollHardfork;

use crate::{LazyLock, ScrollChainConfig, ScrollChainSpec};

/// The Scroll Sepolia spec
pub static SCROLL_SEPOLIA: LazyLock<Arc<ScrollChainSpec>> = LazyLock::new(|| {
    ScrollChainSpec {
        inner: ChainSpec {
            // Let's add TODO(scroll): migrate to Chain::scroll_sepolia() (introduced in https://github.com/alloy-rs/chains/pull/112) when alloy-chains is bumped to version 0.1.48
            chain: Chain::from_named(NamedChain::ScrollSepolia),
            genesis: serde_json::from_str(include_str!("../res/genesis/sepolia_scroll.json"))
                .expect("Can't deserialize Scroll Sepolia genesis json"),
            genesis_hash: once_cell_set(b256!(
                "aa62d1a8b2bffa9e5d2368b63aae0d98d54928bd713125e3fd9e5c896c68592c"
            )),
            hardforks: ScrollHardfork::scroll_sepolia(),
            ..Default::default()
        },
        config: ScrollChainConfig::sepolia(),
    }
    .into()
});
