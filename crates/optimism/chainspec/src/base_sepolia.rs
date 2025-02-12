//! Chain specification for the Base Sepolia testnet network.

use alloc::{sync::Arc, vec};

use alloy_chains::Chain;
use alloy_primitives::U256;
use reth_chainspec::{BaseFeeParams, BaseFeeParamsKind, ChainSpec, Hardfork};
use reth_ethereum_forks::EthereumHardfork;
use reth_optimism_forks::OpHardfork;

use crate::{LazyLock, OpChainSpec};

/// The Base Sepolia spec
pub static BASE_SEPOLIA: LazyLock<Arc<OpChainSpec>> = LazyLock::new(|| {
    OpChainSpec {
        inner: ChainSpec {
            chain: Chain::base_sepolia(),
            genesis: serde_json::from_str(include_str!("../res/genesis/sepolia_base.json"))
                .expect("Can't deserialize Base Sepolia genesis json"),
            paris_block_and_final_difficulty: Some((0, U256::from(0))),
            hardforks: OpHardfork::base_sepolia(),
            base_fee_params: BaseFeeParamsKind::Variable(
                vec![
                    (EthereumHardfork::London.boxed(), BaseFeeParams::base_sepolia()),
                    (OpHardfork::Canyon.boxed(), BaseFeeParams::base_sepolia_canyon()),
                ]
                .into(),
            ),
            prune_delete_limit: 10000,
            ..Default::default()
        },
    }
    .into()
});
