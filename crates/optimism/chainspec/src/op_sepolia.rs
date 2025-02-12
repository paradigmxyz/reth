//! Chain specification for the Optimism Sepolia testnet network.

use crate::{LazyLock, OpChainSpec};
use alloc::{sync::Arc, vec};
use alloy_chains::{Chain, NamedChain};
use alloy_primitives::U256;
use reth_chainspec::{BaseFeeParams, BaseFeeParamsKind, ChainSpec, Hardfork};
use reth_ethereum_forks::EthereumHardfork;
use reth_optimism_forks::OpHardfork;

/// The OP Sepolia spec
pub static OP_SEPOLIA: LazyLock<Arc<OpChainSpec>> = LazyLock::new(|| {
    OpChainSpec {
        inner: ChainSpec {
            chain: Chain::from_named(NamedChain::OptimismSepolia),
            genesis: serde_json::from_str(include_str!("../res/genesis/sepolia_op.json"))
                .expect("Can't deserialize OP Sepolia genesis json"),
            paris_block_and_final_difficulty: Some((0, U256::from(0))),
            hardforks: OpHardfork::op_sepolia(),
            base_fee_params: BaseFeeParamsKind::Variable(
                vec![
                    (EthereumHardfork::London.boxed(), BaseFeeParams::optimism_sepolia()),
                    (OpHardfork::Canyon.boxed(), BaseFeeParams::optimism_sepolia_canyon()),
                ]
                .into(),
            ),
            prune_delete_limit: 10000,
            ..Default::default()
        },
    }
    .into()
});
