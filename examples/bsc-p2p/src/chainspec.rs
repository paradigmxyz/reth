use alloy_primitives::{b256, B256};
use reth_chainspec::{
    once_cell_set, BaseFeeParams, Chain, ChainHardforks, ChainSpec, EthereumHardfork, ForkCondition,
};
use reth_network_peers::NodeRecord;

use std::sync::Arc;

pub const SHANGHAI_TIME: u64 = 1705996800;

pub(crate) fn bsc_chain_spec() -> Arc<ChainSpec> {
    const GENESIS: B256 = b256!("0d21840abff46b96c84b2ac9e10e4f5cdaeb5693cb665db62a2f3b02d2d57b5b");

    ChainSpec {
        chain: Chain::from_id(56),
        genesis: serde_json::from_str(include_str!("./genesis.json")).expect("deserialize genesis"),
        genesis_hash: once_cell_set(GENESIS),
        genesis_header: Default::default(),
        paris_block_and_final_difficulty: None,
        hardforks: ChainHardforks::new(vec![(
            EthereumHardfork::Shanghai.boxed(),
            ForkCondition::Timestamp(SHANGHAI_TIME),
        )]),
        deposit_contract: None,
        base_fee_params: reth_chainspec::BaseFeeParamsKind::Constant(BaseFeeParams::ethereum()),
        max_gas_limit: 140_000_000,
        prune_delete_limit: 0,
    }
    .into()
}

/// BSC mainnet bootnodes <https://github.com/bnb-chain/bsc/blob/master/params/bootnodes.go#L23>
static BOOTNODES: [&str; 7] = [
    "enode://ba88d1a8a5e849bec0eb7df9eabf059f8edeae9a9eb1dcf51b7768276d78b10d4ceecf0cde2ef191ced02f66346d96a36ca9da7d73542757d9677af8da3bad3f@54.198.97.197:30311",
    "enode://433c8bfdf53a3e2268ccb1b829e47f629793291cbddf0c76ae626da802f90532251fc558e2e0d10d6725e759088439bf1cd4714716b03a259a35d4b2e4acfa7f@52.69.102.73:30311",
    "enode://571bee8fb902a625942f10a770ccf727ae2ba1bab2a2b64e121594a99c9437317f6166a395670a00b7d93647eacafe598b6bbcef15b40b6d1a10243865a3e80f@35.73.84.120:30311",
    "enode://fac42fb0ba082b7d1eebded216db42161163d42e4f52c9e47716946d64468a62da4ba0b1cac0df5e8bf1e5284861d757339751c33d51dfef318be5168803d0b5@18.203.152.54:30311",
    "enode://3063d1c9e1b824cfbb7c7b6abafa34faec6bb4e7e06941d218d760acdd7963b274278c5c3e63914bd6d1b58504c59ec5522c56f883baceb8538674b92da48a96@34.250.32.100:30311",
    "enode://ad78c64a4ade83692488aa42e4c94084516e555d3f340d9802c2bf106a3df8868bc46eae083d2de4018f40e8d9a9952c32a0943cd68855a9bc9fd07aac982a6d@34.204.214.24:30311",
    "enode://5db798deb67df75d073f8e2953dad283148133acb520625ea804c9c4ad09a35f13592a762d8f89056248f3889f6dcc33490c145774ea4ff2966982294909b37a@107.20.191.97:30311",
];

pub(crate) fn boot_nodes() -> Vec<NodeRecord> {
    BOOTNODES[..].iter().map(|s| s.parse().unwrap()).collect()
}
