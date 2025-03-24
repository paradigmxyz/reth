//! Chain specification for BSC, credits to: <https://github.com/bnb-chain/reth/blob/main/crates/bsc/chainspec/src/bsc.rs>

use alloy_primitives::{BlockHash, U256};
use hardforks::BscHardfork;
use reth_chainspec::{
    make_genesis_header, BaseFeeParams, BaseFeeParamsKind, Chain, ChainSpec, Head, NamedChain,
};
use reth_primitives::SealedHeader;
use std::{str::FromStr, sync::Arc};

pub mod hardforks;

pub fn bsc_chain_spec() -> Arc<ChainSpec> {
    let genesis = serde_json::from_str(include_str!("genesis.json"))
        .expect("Can't deserialize BSC Mainnet genesis json");
    let hardforks = BscHardfork::bsc_mainnet();
    ChainSpec {
        chain: Chain::from_named(NamedChain::BinanceSmartChain),
        genesis: serde_json::from_str(include_str!("genesis.json"))
            .expect("Can't deserialize BSC Mainnet genesis json"),
        paris_block_and_final_difficulty: Some((0, U256::from(0))),
        hardforks: BscHardfork::bsc_mainnet(),
        deposit_contract: None,
        base_fee_params: BaseFeeParamsKind::Constant(BaseFeeParams::new(1, 1)),
        prune_delete_limit: 3500,
        genesis_header: SealedHeader::new(
            make_genesis_header(&genesis, &hardforks),
            BlockHash::from_str(
                "0x0d21840abff46b96c84b2ac9e10e4f5cdaeb5693cb665db62a2f3b02d2d57b5b",
            )
            .unwrap(),
        ),
        ..Default::default()
    }
    .into()
}

pub fn head() -> Head {
    Head { number: 40_000_000, timestamp: 1742436600, ..Default::default() }
}

#[cfg(test)]
mod tests {
    use crate::chainspec::{bsc_chain_spec, head};
    use alloy_primitives::hex;
    use reth_chainspec::{ForkHash, ForkId};

    #[test]
    fn can_create_forkid() {
        let b = hex::decode("ce18f5d3").unwrap();
        let expected = [b[0], b[1], b[2], b[3]];
        let expected_f_id = ForkId { hash: ForkHash(expected), next: 0 };

        let fork_id = bsc_chain_spec().fork_id(&head());
        assert_eq!(fork_id, expected_f_id);
    }
}
