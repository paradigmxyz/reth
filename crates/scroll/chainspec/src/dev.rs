//! Chain specification in dev mode for custom chain.

use crate::{
    constants::SCROLL_BASE_FEE_PARAMS_FEYNMAN, make_genesis_header, LazyLock, ScrollChainConfig,
    ScrollChainSpec,
};
use alloc::{sync::Arc, vec};

use alloy_chains::Chain;
use alloy_primitives::U256;
use reth_chainspec::{BaseFeeParamsKind, ChainSpec, Hardfork};
use reth_primitives_traits::SealedHeader;
use reth_scroll_forks::DEV_HARDFORKS;
use scroll_alloy_hardforks::ScrollHardfork;

/// Scroll dev testnet specification
///
/// Includes 20 prefunded accounts with `10_000` ETH each derived from mnemonic "test test test test
/// test test test test test test test junk".
pub static SCROLL_DEV: LazyLock<Arc<ScrollChainSpec>> = LazyLock::new(|| {
    let genesis = serde_json::from_str(include_str!("../res/genesis/dev.json"))
        .expect("Can't deserialize Dev testnet genesis json");
    ScrollChainSpec {
        inner: ChainSpec {
            chain: Chain::dev(),
            genesis_header: SealedHeader::new_unhashed(make_genesis_header(&genesis)),
            genesis,
            paris_block_and_final_difficulty: Some((0, U256::from(0))),
            hardforks: DEV_HARDFORKS.clone(),
            base_fee_params: BaseFeeParamsKind::Variable(
                vec![(ScrollHardfork::Feynman.boxed(), SCROLL_BASE_FEE_PARAMS_FEYNMAN)].into(),
            ),
            ..Default::default()
        },
        config: ScrollChainConfig::dev(),
    }
    .into()
});
