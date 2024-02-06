use reth_primitives::{
    b256, BaseFeeParams, Chain, ChainSpec, ForkCondition, ForkTimestamps, Hardfork, Head,
    NodeRecord, B256,
};

use std::{collections::BTreeMap, sync::Arc};

const SHANGAI_BLOCK: u64 = 50523000;

pub(crate) fn polygon_chain_spec() -> Arc<ChainSpec> {
    const GENESIS: B256 = b256!("a9c28ce2141b56c474f1dc504bee9b01eb1bd7d1a507580d5519d4437a97de1b");

    ChainSpec {
        chain: Chain::from_id(137),
        // <https://github.com/maticnetwork/bor/blob/d521b8e266b97efe9c8fdce8167e9dd77b04637d/builder/files/genesis-mainnet-v1.json>
        genesis: serde_json::from_str(include_str!("./genesis.json")).expect("deserialize genesis"),
        genesis_hash: Some(GENESIS),
        fork_timestamps: ForkTimestamps::default().shanghai(1681338455),
        paris_block_and_final_difficulty: None,
        hardforks: BTreeMap::from([
            (Hardfork::Petersburg, ForkCondition::Block(0)),
            (Hardfork::Istanbul, ForkCondition::Block(3395000)),
            (Hardfork::MuirGlacier, ForkCondition::Block(3395000)),
            (Hardfork::Berlin, ForkCondition::Block(14750000)),
            (Hardfork::London, ForkCondition::Block(23850000)),
            (Hardfork::Shanghai, ForkCondition::Block(SHANGAI_BLOCK)),
        ]),
        deposit_contract: None,
        base_fee_params: reth_primitives::BaseFeeParamsKind::Constant(BaseFeeParams::ethereum()),
        prune_delete_limit: 0,
    }
    .into()
}

/// Polygon mainnet boot nodes <https://github.com/maticnetwork/bor/blob/master/params/bootnodes.go#L79>
static BOOTNODES : [&str; 4] = [
	"enode://b8f1cc9c5d4403703fbf377116469667d2b1823c0daf16b7250aa576bacf399e42c3930ccfcb02c5df6879565a2b8931335565f0e8d3f8e72385ecf4a4bf160a@3.36.224.80:30303",
	"enode://8729e0c825f3d9cad382555f3e46dcff21af323e89025a0e6312df541f4a9e73abfa562d64906f5e59c51fe6f0501b3e61b07979606c56329c020ed739910759@54.194.245.5:30303",
	"enode://76316d1cb93c8ed407d3332d595233401250d48f8fbb1d9c65bd18c0495eca1b43ec38ee0ea1c257c0abb7d1f25d649d359cdfe5a805842159cfe36c5f66b7e8@52.78.36.216:30303",
	"enode://681ebac58d8dd2d8a6eef15329dfbad0ab960561524cf2dfde40ad646736fe5c244020f20b87e7c1520820bc625cfb487dd71d63a3a3bf0baea2dbb8ec7c79f1@34.240.245.39:30303",
];

pub(crate) fn head() -> Head {
    Head { number: SHANGAI_BLOCK, ..Default::default() }
}

pub(crate) fn boot_nodes() -> Vec<NodeRecord> {
    BOOTNODES[..].iter().map(|s| s.parse().unwrap()).collect()
}
