//! Chain specification for the Scroll Mainnet network.

use crate::{
    constants::SCROLL_BASE_FEE_PARAMS_FEYNMAN, make_genesis_header, LazyLock, ScrollChainConfig,
    ScrollChainSpec, SCROLL_MAINNET_GENESIS_HASH,
};
use alloc::{sync::Arc, vec};

use alloy_chains::{Chain, NamedChain};
use reth_chainspec::{BaseFeeParamsKind, ChainSpec, Hardfork};
use reth_primitives_traits::SealedHeader;
use reth_scroll_forks::SCROLL_MAINNET_HARDFORKS;
use scroll_alloy_hardforks::ScrollHardfork;

/// The Scroll Mainnet spec
pub static SCROLL_MAINNET: LazyLock<Arc<ScrollChainSpec>> = LazyLock::new(|| {
    let genesis = serde_json::from_str(include_str!("../res/genesis/scroll.json"))
        .expect("Can't deserialize Scroll Mainnet genesis json");
    ScrollChainSpec {
        inner: ChainSpec {
            // TODO(scroll): migrate to Chain::scroll() (introduced in https://github.com/alloy-rs/chains/pull/112) when alloy-chains is bumped to version 0.1.48
            chain: Chain::from_named(NamedChain::Scroll),
            genesis_header: SealedHeader::new(
                make_genesis_header(&genesis),
                SCROLL_MAINNET_GENESIS_HASH,
            ),
            genesis,
            hardforks: SCROLL_MAINNET_HARDFORKS.clone(),
            base_fee_params: BaseFeeParamsKind::Variable(
                vec![(ScrollHardfork::Feynman.boxed(), SCROLL_BASE_FEE_PARAMS_FEYNMAN)].into(),
            ),
            ..Default::default()
        },
        config: ScrollChainConfig::mainnet(),
    }
    .into()
});
