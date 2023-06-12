use std::{sync::Arc, collections::BTreeMap};

use crate::{ChainSpec, Chain, H256, U256, Hardfork, ForkCondition, hex_literal::hex};


use once_cell::sync::Lazy;

use super::spec::ForkTimestamps;

// use super::spec::ForkTimestamps;

/// The Ethereum mainnet spec
// FIXME: This is dummy data just to get rid of compile errors
pub static MAINNET_SPEC: Lazy<Arc<ChainSpec>> = Lazy::new(|| {
    ChainSpec {
        chain: Chain::mainnet(),
        genesis: serde_json::from_str(include_str!("../../res/genesis/mainnet.json"))
            .expect("Can't deserialize Mainnet genesis json"),
        genesis_hash: Some(H256(hex!(
            "d4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3"
        ))),
        // <https://etherscan.io/block/15537394>
        paris_block_and_final_difficulty: Some((
            15537394,
            U256::from(58_750_003_716_598_352_816_469u128),
        )),
        fork_timestamps: ForkTimestamps::default().shanghai(1681338455),
        hardforks: BTreeMap::from([
            (Hardfork::Frontier, ForkCondition::Block(0)),
        ]),
    }
    .into()
});

// /// The Goerli spec
// pub static GOERLI: Lazy<Arc<ChainSpec>> = Lazy::new(|| {
//     ChainSpec {
//         chain: Chain::goerli(),
//         genesis: serde_json::from_str(include_str!("../../res/genesis/goerli.json"))
//             .expect("Can't deserialize Goerli genesis json"),
//         genesis_hash: Some(H256(hex!(
//             "bf7e331f7f7c1dd2e05159666b3bf8bc7a8a3a9eb1d518969eab529dd9b88c1a"
//         ))),
//         // <https://goerli.etherscan.io/block/7382818>
//         paris_block_and_final_difficulty: Some((7382818, U256::from(10_790_000))),
//         fork_timestamps: ForkTimestamps::default().shanghai(1678832736),
//         hardforks: BTreeMap::from([
//             (Hardfork::Frontier, ForkCondition::Block(0)),
//             (Hardfork::Homestead, ForkCondition::Block(0)),
//             (Hardfork::Dao, ForkCondition::Block(0)),
//             (Hardfork::Tangerine, ForkCondition::Block(0)),
//             (Hardfork::SpuriousDragon, ForkCondition::Block(0)),
//             (Hardfork::Byzantium, ForkCondition::Block(0)),
//             (Hardfork::Constantinople, ForkCondition::Block(0)),
//             (Hardfork::Petersburg, ForkCondition::Block(0)),
//             (Hardfork::Istanbul, ForkCondition::Block(1561651)),
//             (Hardfork::Berlin, ForkCondition::Block(4460644)),
//             (Hardfork::London, ForkCondition::Block(5062605)),
//             (
//                 Hardfork::Paris,
//                 ForkCondition::TTD { fork_block: None, total_difficulty: U256::from(10_790_000) },
//             ),
//             (Hardfork::Shanghai, ForkCondition::Timestamp(1678832736)),
//         ]),
//     }
//     .into()
// });

// /// The Sepolia spec
// pub static SEPOLIA: Lazy<Arc<ChainSpec>> = Lazy::new(|| {
//     ChainSpec {
//         chain: Chain::sepolia(),
//         genesis: serde_json::from_str(include_str!("../../res/genesis/sepolia.json"))
//             .expect("Can't deserialize Sepolia genesis json"),
//         genesis_hash: Some(H256(hex!(
//             "25a5cc106eea7138acab33231d7160d69cb777ee0c2c553fcddf5138993e6dd9"
//         ))),
//         // <https://sepolia.etherscan.io/block/1450409>
//         paris_block_and_final_difficulty: Some((1450409, U256::from(17_000_018_015_853_232u128))),
//         fork_timestamps: ForkTimestamps::default().shanghai(1677557088),
//         hardforks: BTreeMap::from([
//             (Hardfork::Frontier, ForkCondition::Block(0)),
//             (Hardfork::Homestead, ForkCondition::Block(0)),
//             (Hardfork::Dao, ForkCondition::Block(0)),
//             (Hardfork::Tangerine, ForkCondition::Block(0)),
//             (Hardfork::SpuriousDragon, ForkCondition::Block(0)),
//             (Hardfork::Byzantium, ForkCondition::Block(0)),
//             (Hardfork::Constantinople, ForkCondition::Block(0)),
//             (Hardfork::Petersburg, ForkCondition::Block(0)),
//             (Hardfork::Istanbul, ForkCondition::Block(0)),
//             (Hardfork::MuirGlacier, ForkCondition::Block(0)),
//             (Hardfork::Berlin, ForkCondition::Block(0)),
//             (Hardfork::London, ForkCondition::Block(0)),
//             (
//                 Hardfork::Paris,
//                 ForkCondition::TTD {
//                     fork_block: Some(1735371),
//                     total_difficulty: U256::from(17_000_000_000_000_000u64),
//                 },
//             ),
//             (Hardfork::Shanghai, ForkCondition::Timestamp(1677557088)),
//         ]),
//     }
//     .into()
// });