//! Chain specification for the Unichain Mainnet network.

use crate::{LazyLock, OpChainSpec};
use alloc::{sync::Arc, vec};
use alloy_chains::{Chain, NamedChain};
use alloy_eips::eip1559::BaseFeeParams;
use alloy_primitives::{b256, U256};
use reth_chainspec::{once_cell_set, BaseFeeParamsKind, ChainSpec};
use reth_ethereum_forks::Hardfork;
use reth_optimism_forks::OpHardfork;

/// The Unichain mainnet spec
pub static UNICHAIN_MAINNET: LazyLock<Arc<OpChainSpec>> = LazyLock::new(|| {
    OpChainSpec {
        inner: ChainSpec {
            chain: Chain::from_named(NamedChain::Unichain),
            genesis: serde_json::from_str(include_str!("../res/genesis/unichain.json"))
                .expect("Can't deserialize Unichain genesis json"),
            genesis_hash: once_cell_set(b256!(
                "0x3425162ddf41a0a1f0106d67b71828c9a9577e6ddeb94e4f33d2cde1fdc3befe"
            )),
            paris_block_and_final_difficulty: Some((0, U256::from(0))),
            hardforks: OpHardfork::unichain_mainnet(),
            base_fee_params: BaseFeeParamsKind::Variable(
                vec![(OpHardfork::Canyon.boxed(), BaseFeeParams::optimism_canyon())].into(),
            ),
            ..Default::default()
        },
    }
    .into()
});
