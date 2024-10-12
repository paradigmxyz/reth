//! Chain specification for the Base Sepolia testnet network.

#[cfg(not(feature = "std"))]
use alloc::sync::Arc;
#[cfg(feature = "std")]
use std::sync::Arc;

use alloy_chains::Chain;
use alloy_primitives::{b256, U256};
use once_cell::sync::Lazy;
use reth_chainspec::{BaseFeeParams, BaseFeeParamsKind, ChainSpec};
use reth_ethereum_forks::{EthereumHardfork, OptimismHardfork};

use crate::OpChainSpec;

/// The Base Sepolia spec
pub static BASE_SEPOLIA: Lazy<Arc<OpChainSpec>> = Lazy::new(|| {
    OpChainSpec {
        inner: ChainSpec {
            chain: Chain::base_sepolia(),
            genesis: serde_json::from_str(include_str!("../res/genesis/sepolia_base.json"))
                .expect("Can't deserialize Base Sepolia genesis json"),
            genesis_hash: Some(b256!(
                "0dcc9e089e30b90ddfc55be9a37dd15bc551aeee999d2e2b51414c54eaf934e4"
            )),
            paris_block_and_final_difficulty: Some((0, U256::from(0))),
            hardforks: OptimismHardfork::base_sepolia(),
            base_fee_params: BaseFeeParamsKind::Variable(
                vec![
                    (EthereumHardfork::London.boxed(), BaseFeeParams::base_sepolia()),
                    (OptimismHardfork::Canyon.boxed(), BaseFeeParams::base_sepolia_canyon()),
                ]
                .into(),
            ),
            max_gas_limit: crate::constants::BASE_SEPOLIA_MAX_GAS_LIMIT,
            prune_delete_limit: 10000,
            ..Default::default()
        },
    }
    .into()
});
