//! Chain specification for the Scroll Mainnet network.

use alloc::sync::Arc;

use alloy_chains::{Chain, NamedChain};
use alloy_primitives::b256;
use reth_chainspec::{make_genesis_header, ChainSpec};
use reth_primitives_traits::SealedHeader;
use reth_scroll_forks::ScrollHardfork;

use crate::{LazyLock, ScrollChainConfig, ScrollChainSpec};

/// The Scroll Mainnet spec
pub static SCROLL_MAINNET: LazyLock<Arc<ScrollChainSpec>> = LazyLock::new(|| {
    let genesis = serde_json::from_str(include_str!("../res/genesis/scroll.json"))
        .expect("Can't deserialize Scroll Mainnet genesis json");
    let hardforks = ScrollHardfork::scroll_mainnet();
    ScrollChainSpec {
        inner: ChainSpec {
            // TODO(scroll): migrate to Chain::scroll() (introduced in https://github.com/alloy-rs/chains/pull/112) when alloy-chains is bumped to version 0.1.48
            chain: Chain::from_named(NamedChain::Scroll),
            genesis_header: SealedHeader::new(
                make_genesis_header(&genesis, &hardforks),
                b256!("bbc05efd412b7cd47a2ed0e5ddfcf87af251e414ea4c801d78b6784513180a80"),
            ),
            genesis,
            hardforks,
            ..Default::default()
        },
        config: ScrollChainConfig::mainnet(),
    }
    .into()
});
