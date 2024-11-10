//! Chain specification for the Optimism Sepolia testnet network.

use alloc::{sync::Arc, vec};

use alloy_chains::{Chain, NamedChain};
use alloy_eips::eip1559::ETHEREUM_BLOCK_GAS_LIMIT;
use alloy_primitives::{b256, U256};
use reth_chainspec::{once_cell_set, BaseFeeParams, BaseFeeParamsKind, ChainSpec};
use reth_ethereum_forks::EthereumHardfork;
use reth_optimism_forks::OpHardfork;

use crate::{LazyLock, OpChainSpec};

/// The OP Sepolia spec
pub static OP_SEPOLIA: LazyLock<Arc<OpChainSpec>> = LazyLock::new(|| {
    OpChainSpec {
        inner: ChainSpec {
            chain: Chain::from_named(NamedChain::OptimismSepolia),
            genesis: serde_json::from_str(include_str!("../res/genesis/sepolia_op.json"))
                .expect("Can't deserialize OP Sepolia genesis json"),
            genesis_hash: once_cell_set(b256!(
                "102de6ffb001480cc9b8b548fd05c34cd4f46ae4aa91759393db90ea0409887d"
            )),
            paris_block_and_final_difficulty: Some((0, U256::from(0))),
            hardforks: OpHardfork::op_sepolia(),
            base_fee_params: BaseFeeParamsKind::Variable(
                vec![
                    (EthereumHardfork::London.boxed(), BaseFeeParams::optimism_sepolia()),
                    (OpHardfork::Canyon.boxed(), BaseFeeParams::optimism_sepolia_canyon()),
                ]
                .into(),
            ),
            max_gas_limit: ETHEREUM_BLOCK_GAS_LIMIT,
            prune_delete_limit: 10000,
            ..Default::default()
        },
    }
    .into()
});
