//! Chain specification for the Base Sepolia testnet network.

use alloc::{sync::Arc, vec};

use alloy_chains::Chain;
use alloy_primitives::{b256, U256};
use reth_chainspec::{once_cell_set, BaseFeeParams, BaseFeeParamsKind, ChainSpec};
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
            genesis_hash: once_cell_set(b256!(
                "0dcc9e089e30b90ddfc55be9a37dd15bc551aeee999d2e2b51414c54eaf934e4"
            )),
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
