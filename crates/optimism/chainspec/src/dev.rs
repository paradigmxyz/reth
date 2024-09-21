//! Chain specification in dev mode for custom chain.

use alloc::sync::Arc;

use alloy_chains::Chain;
use alloy_primitives::U256;
use once_cell::sync::Lazy;
use reth_chainspec::{once_cell_set, BaseFeeParams, BaseFeeParamsKind, ChainSpec};
use reth_optimism_forks::DEV_HARDFORKS;
use reth_primitives_traits::constants::DEV_GENESIS_HASH;

use crate::OpChainSpec;

/// OP dev testnet specification
///
/// Includes 20 prefunded accounts with `10_000` ETH each derived from mnemonic "test test test test
/// test test test test test test test junk".
pub static OP_DEV: Lazy<Arc<OpChainSpec>> = Lazy::new(|| {
    OpChainSpec {
        inner: ChainSpec {
            chain: Chain::dev(),
            genesis: serde_json::from_str(include_str!("../res/genesis/dev.json"))
                .expect("Can't deserialize Dev testnet genesis json"),
            genesis_hash: once_cell_set(DEV_GENESIS_HASH),
            paris_block_and_final_difficulty: Some((0, U256::from(0))),
            hardforks: DEV_HARDFORKS.clone(),
            base_fee_params: BaseFeeParamsKind::Constant(BaseFeeParams::ethereum()),
            deposit_contract: None, // TODO: do we even have?
            ..Default::default()
        },
    }
    .into()
});
