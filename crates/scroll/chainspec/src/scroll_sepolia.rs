//! Chain specification for the Scroll Sepolia testnet network.

use crate::{make_genesis_header, LazyLock, ScrollChainConfig, ScrollChainSpec};
use alloc::sync::Arc;

use alloy_chains::{Chain, NamedChain};
use alloy_primitives::b256;
use reth_chainspec::ChainSpec;
use reth_primitives_traits::SealedHeader;
use reth_scroll_forks::ScrollHardfork;

/// The Scroll Sepolia spec
pub static SCROLL_SEPOLIA: LazyLock<Arc<ScrollChainSpec>> = LazyLock::new(|| {
    let genesis = serde_json::from_str(include_str!("../res/genesis/sepolia_scroll.json"))
        .expect("Can't deserialize Scroll Sepolia genesis json");
    ScrollChainSpec {
        inner: ChainSpec {
            // TODO(scroll): migrate to Chain::scroll_sepolia() (introduced in https://github.com/alloy-rs/chains/pull/112) when alloy-chains is bumped to version 0.1.48
            chain: Chain::from_named(NamedChain::ScrollSepolia),
            genesis_header: SealedHeader::new(
                make_genesis_header(&genesis),
                b256!("aa62d1a8b2bffa9e5d2368b63aae0d98d54928bd713125e3fd9e5c896c68592c"),
            ),
            genesis,
            hardforks: ScrollHardfork::scroll_sepolia(),
            ..Default::default()
        },
        config: ScrollChainConfig::sepolia(),
    }
    .into()
});
