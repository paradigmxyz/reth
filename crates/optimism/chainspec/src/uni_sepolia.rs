//! Chain specification for the Base Sepolia testnet network.

use alloc::{sync::Arc, vec};

use alloy_chains::Chain;
use alloy_primitives::{b256, U256};
use reth_chainspec::{BaseFeeParams, BaseFeeParamsKind, ChainSpec, Hardfork};
use reth_ethereum_forks::EthereumHardfork;
use reth_optimism_forks::{OpHardfork, UNICHAIN_SEPOLIA_HARDFORKS};
use reth_primitives_traits::SealedHeader;

use crate::{make_op_genesis_header, LazyLock, OpChainSpec};

/// The Unichain Sepolia spec
pub static UNICHAIN_SEPOLIA: LazyLock<Arc<OpChainSpec>> = LazyLock::new(|| {
    let genesis = serde_json::from_str(include_str!("../res/genesis/sepolia_uni.json"))
        .expect("Can't deserialize Unichain Sepolia genesis json");
    let hardforks = UNICHAIN_SEPOLIA_HARDFORKS.clone();
    OpChainSpec {
        inner: ChainSpec {
            chain: Chain::unichain_sepolia(),
            genesis_header: SealedHeader::new(
                make_op_genesis_header(&genesis, &hardforks),
                b256!("0xb7fe0bc9f98ca03294ca0094ff9374cc3e64130b6ec85850d6e260191f48bfe7"),
            ),
            genesis,
            paris_block_and_final_difficulty: Some((0, U256::from(0))),
            hardforks,
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
