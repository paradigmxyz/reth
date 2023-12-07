use crate::{
    constants::{
        EIP1559_DEFAULT_BASE_FEE_MAX_CHANGE_DENOMINATOR, EIP1559_DEFAULT_ELASTICITY_MULTIPLIER,
        EIP1559_INITIAL_BASE_FEE, EMPTY_RECEIPTS, EMPTY_TRANSACTIONS, EMPTY_WITHDRAWALS,
    },
    proofs::state_root_ref_unhashed,
    revm_primitives::{address, b256},
    Address, BlockNumber, Chain, ForkFilter, ForkFilterKey, ForkHash, ForkId, Genesis, Hardfork,
    Head, Header, SealedHeader, B256, EMPTY_OMMER_ROOT_HASH, U256,
};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fmt::{Display, Formatter},
    sync::Arc,
};

/// The Ethereum mainnet spec
pub static MAINNET: Lazy<Arc<ChainSpec>> = Lazy::new(|| {
    ChainSpec {
        chain: Chain::mainnet(),
        genesis: serde_json::from_str(include_str!("../../res/genesis/mainnet.json"))
            .expect("Can't deserialize Mainnet genesis json"),
        genesis_hash: Some(b256!(
            "d4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3"
        )),
        // <https://etherscan.io/block/15537394>
        paris_block_and_final_difficulty: Some((
            15537394,
            U256::from(58_750_003_716_598_352_816_469u128),
        )),
        fork_timestamps: ForkTimestamps::default().shanghai(1681338455),
        hardforks: BTreeMap::from([
            (Hardfork::Frontier, ForkCondition::Block(0)),
            (Hardfork::Homestead, ForkCondition::Block(1150000)),
            (Hardfork::Dao, ForkCondition::Block(1920000)),
            (Hardfork::Tangerine, ForkCondition::Block(2463000)),
            (Hardfork::SpuriousDragon, ForkCondition::Block(2675000)),
            (Hardfork::Byzantium, ForkCondition::Block(4370000)),
            (Hardfork::Constantinople, ForkCondition::Block(7280000)),
            (Hardfork::Petersburg, ForkCondition::Block(7280000)),
            (Hardfork::Istanbul, ForkCondition::Block(9069000)),
            (Hardfork::MuirGlacier, ForkCondition::Block(9200000)),
            (Hardfork::Berlin, ForkCondition::Block(12244000)),
            (Hardfork::London, ForkCondition::Block(12965000)),
            (Hardfork::ArrowGlacier, ForkCondition::Block(13773000)),
            (Hardfork::GrayGlacier, ForkCondition::Block(15050000)),
            (
                Hardfork::Paris,
                ForkCondition::TTD {
                    fork_block: None,
                    total_difficulty: U256::from(58_750_000_000_000_000_000_000_u128),
                },
            ),
            (Hardfork::Shanghai, ForkCondition::Timestamp(1681338455)),
        ]),
        // https://etherscan.io/tx/0xe75fb554e433e03763a1560646ee22dcb74e5274b34c5ad644e7c0f619a7e1d0
        deposit_contract: Some(DepositContract::new(
            address!("00000000219ab540356cbb839cbe05303d7705fa"),
            11052984,
            b256!("649bbc62d0e31342afea4e5cd82d4049e7e1ee912fc0889aa790803be39038c5"),
        )),
        base_fee_params: BaseFeeParamsKind::Constant(BaseFeeParams::ethereum()),
        prune_delete_limit: 3500,
        snapshot_block_interval: 500_000,
    }
    .into()
});

/// The Goerli spec
pub static GOERLI: Lazy<Arc<ChainSpec>> = Lazy::new(|| {
    ChainSpec {
        chain: Chain::goerli(),
        genesis: serde_json::from_str(include_str!("../../res/genesis/goerli.json"))
            .expect("Can't deserialize Goerli genesis json"),
        genesis_hash: Some(b256!(
            "bf7e331f7f7c1dd2e05159666b3bf8bc7a8a3a9eb1d518969eab529dd9b88c1a"
        )),
        // <https://goerli.etherscan.io/block/7382818>
        paris_block_and_final_difficulty: Some((7382818, U256::from(10_790_000))),
        fork_timestamps: ForkTimestamps::default().shanghai(1678832736),
        hardforks: BTreeMap::from([
            (Hardfork::Frontier, ForkCondition::Block(0)),
            (Hardfork::Homestead, ForkCondition::Block(0)),
            (Hardfork::Dao, ForkCondition::Block(0)),
            (Hardfork::Tangerine, ForkCondition::Block(0)),
            (Hardfork::SpuriousDragon, ForkCondition::Block(0)),
            (Hardfork::Byzantium, ForkCondition::Block(0)),
            (Hardfork::Constantinople, ForkCondition::Block(0)),
            (Hardfork::Petersburg, ForkCondition::Block(0)),
            (Hardfork::Istanbul, ForkCondition::Block(1561651)),
            (Hardfork::Berlin, ForkCondition::Block(4460644)),
            (Hardfork::London, ForkCondition::Block(5062605)),
            (
                Hardfork::Paris,
                ForkCondition::TTD { fork_block: None, total_difficulty: U256::from(10_790_000) },
            ),
            (Hardfork::Shanghai, ForkCondition::Timestamp(1678832736)),
        ]),
        // https://goerli.etherscan.io/tx/0xa3c07dc59bfdb1bfc2d50920fed2ef2c1c4e0a09fe2325dbc14e07702f965a78
        deposit_contract: Some(DepositContract::new(
            address!("ff50ed3d0ec03ac01d4c79aad74928bff48a7b2b"),
            4367322,
            b256!("649bbc62d0e31342afea4e5cd82d4049e7e1ee912fc0889aa790803be39038c5"),
        )),
        base_fee_params: BaseFeeParamsKind::Constant(BaseFeeParams::ethereum()),
        prune_delete_limit: 1700,
        snapshot_block_interval: 1_000_000,
    }
    .into()
});

/// The Sepolia spec
pub static SEPOLIA: Lazy<Arc<ChainSpec>> = Lazy::new(|| {
    ChainSpec {
        chain: Chain::sepolia(),
        genesis: serde_json::from_str(include_str!("../../res/genesis/sepolia.json"))
            .expect("Can't deserialize Sepolia genesis json"),
        genesis_hash: Some(b256!(
            "25a5cc106eea7138acab33231d7160d69cb777ee0c2c553fcddf5138993e6dd9"
        )),
        // <https://sepolia.etherscan.io/block/1450409>
        paris_block_and_final_difficulty: Some((1450409, U256::from(17_000_018_015_853_232u128))),
        fork_timestamps: ForkTimestamps::default().shanghai(1677557088),
        hardforks: BTreeMap::from([
            (Hardfork::Frontier, ForkCondition::Block(0)),
            (Hardfork::Homestead, ForkCondition::Block(0)),
            (Hardfork::Dao, ForkCondition::Block(0)),
            (Hardfork::Tangerine, ForkCondition::Block(0)),
            (Hardfork::SpuriousDragon, ForkCondition::Block(0)),
            (Hardfork::Byzantium, ForkCondition::Block(0)),
            (Hardfork::Constantinople, ForkCondition::Block(0)),
            (Hardfork::Petersburg, ForkCondition::Block(0)),
            (Hardfork::Istanbul, ForkCondition::Block(0)),
            (Hardfork::MuirGlacier, ForkCondition::Block(0)),
            (Hardfork::Berlin, ForkCondition::Block(0)),
            (Hardfork::London, ForkCondition::Block(0)),
            (
                Hardfork::Paris,
                ForkCondition::TTD {
                    fork_block: Some(1735371),
                    total_difficulty: U256::from(17_000_000_000_000_000u64),
                },
            ),
            (Hardfork::Shanghai, ForkCondition::Timestamp(1677557088)),
        ]),
        // https://sepolia.etherscan.io/tx/0x025ecbf81a2f1220da6285d1701dc89fb5a956b62562ee922e1a9efd73eb4b14
        deposit_contract: Some(DepositContract::new(
            address!("7f02c3e3c98b133055b8b348b2ac625669ed295d"),
            1273020,
            b256!("649bbc62d0e31342afea4e5cd82d4049e7e1ee912fc0889aa790803be39038c5"),
        )),
        base_fee_params: BaseFeeParamsKind::Constant(BaseFeeParams::ethereum()),
        prune_delete_limit: 1700,
        snapshot_block_interval: 1_000_000,
    }
    .into()
});

/// The Holesky spec
pub static HOLESKY: Lazy<Arc<ChainSpec>> = Lazy::new(|| {
    ChainSpec {
        chain: Chain::holesky(),
        genesis: serde_json::from_str(include_str!("../../res/genesis/holesky.json"))
            .expect("Can't deserialize Holesky genesis json"),
        genesis_hash: Some(b256!(
            "b5f7f912443c940f21fd611f12828d75b534364ed9e95ca4e307729a4661bde4"
        )),
        paris_block_and_final_difficulty: Some((0, U256::from(1))),
        fork_timestamps: ForkTimestamps::default().shanghai(1696000704),
        hardforks: BTreeMap::from([
            (Hardfork::Frontier, ForkCondition::Block(0)),
            (Hardfork::Homestead, ForkCondition::Block(0)),
            (Hardfork::Dao, ForkCondition::Block(0)),
            (Hardfork::Tangerine, ForkCondition::Block(0)),
            (Hardfork::SpuriousDragon, ForkCondition::Block(0)),
            (Hardfork::Byzantium, ForkCondition::Block(0)),
            (Hardfork::Constantinople, ForkCondition::Block(0)),
            (Hardfork::Petersburg, ForkCondition::Block(0)),
            (Hardfork::Istanbul, ForkCondition::Block(0)),
            (Hardfork::MuirGlacier, ForkCondition::Block(0)),
            (Hardfork::Berlin, ForkCondition::Block(0)),
            (Hardfork::London, ForkCondition::Block(0)),
            (
                Hardfork::Paris,
                ForkCondition::TTD { fork_block: Some(0), total_difficulty: U256::ZERO },
            ),
            (Hardfork::Shanghai, ForkCondition::Timestamp(1696000704)),
        ]),
        deposit_contract: Some(DepositContract::new(
            address!("4242424242424242424242424242424242424242"),
            0,
            b256!("649bbc62d0e31342afea4e5cd82d4049e7e1ee912fc0889aa790803be39038c5"),
        )),
        base_fee_params: BaseFeeParamsKind::Constant(BaseFeeParams::ethereum()),
        prune_delete_limit: 1700,
        snapshot_block_interval: 1_000_000,
    }
    .into()
});

/// Dev testnet specification
///
/// Includes 20 prefunded accounts with 10_000 ETH each derived from mnemonic "test test test test
/// test test test test test test test junk".
pub static DEV: Lazy<Arc<ChainSpec>> = Lazy::new(|| {
    ChainSpec {
        chain: Chain::dev(),
        genesis: serde_json::from_str(include_str!("../../res/genesis/dev.json"))
            .expect("Can't deserialize Dev testnet genesis json"),
        genesis_hash: Some(b256!(
            "2f980576711e3617a5e4d83dd539548ec0f7792007d505a3d2e9674833af2d7c"
        )),
        paris_block_and_final_difficulty: Some((0, U256::from(0))),
        fork_timestamps: ForkTimestamps::default().shanghai(0),
        hardforks: BTreeMap::from([
            (Hardfork::Frontier, ForkCondition::Block(0)),
            (Hardfork::Homestead, ForkCondition::Block(0)),
            (Hardfork::Dao, ForkCondition::Block(0)),
            (Hardfork::Tangerine, ForkCondition::Block(0)),
            (Hardfork::SpuriousDragon, ForkCondition::Block(0)),
            (Hardfork::Byzantium, ForkCondition::Block(0)),
            (Hardfork::Constantinople, ForkCondition::Block(0)),
            (Hardfork::Petersburg, ForkCondition::Block(0)),
            (Hardfork::Istanbul, ForkCondition::Block(0)),
            (Hardfork::MuirGlacier, ForkCondition::Block(0)),
            (Hardfork::Berlin, ForkCondition::Block(0)),
            (Hardfork::London, ForkCondition::Block(0)),
            (
                Hardfork::Paris,
                ForkCondition::TTD { fork_block: Some(0), total_difficulty: U256::from(0) },
            ),
            (Hardfork::Shanghai, ForkCondition::Timestamp(0)),
        ]),
        base_fee_params: BaseFeeParamsKind::Constant(BaseFeeParams::ethereum()),
        deposit_contract: None, // TODO: do we even have?
        ..Default::default()
    }
    .into()
});

/// The Optimism Goerli spec
#[cfg(feature = "optimism")]
pub static OP_GOERLI: Lazy<Arc<ChainSpec>> = Lazy::new(|| {
    ChainSpec {
        chain: Chain::optimism_goerli(),
        genesis: serde_json::from_str(include_str!("../../res/genesis/goerli_op.json"))
            .expect("Can't deserialize Optimism Goerli genesis json"),
        genesis_hash: Some(b256!(
            "c1fc15cd51159b1f1e5cbc4b82e85c1447ddfa33c52cf1d98d14fba0d6354be1"
        )),
        fork_timestamps: ForkTimestamps::default().shanghai(1699981200).canyon(1699981200),
        paris_block_and_final_difficulty: Some((0, U256::from(0))),
        hardforks: BTreeMap::from([
            (Hardfork::Frontier, ForkCondition::Block(0)),
            (Hardfork::Homestead, ForkCondition::Block(0)),
            (Hardfork::Tangerine, ForkCondition::Block(0)),
            (Hardfork::SpuriousDragon, ForkCondition::Block(0)),
            (Hardfork::Byzantium, ForkCondition::Block(0)),
            (Hardfork::Constantinople, ForkCondition::Block(0)),
            (Hardfork::Petersburg, ForkCondition::Block(0)),
            (Hardfork::Istanbul, ForkCondition::Block(0)),
            (Hardfork::MuirGlacier, ForkCondition::Block(0)),
            (Hardfork::Berlin, ForkCondition::Block(0)),
            (Hardfork::London, ForkCondition::Block(0)),
            (Hardfork::ArrowGlacier, ForkCondition::Block(0)),
            (Hardfork::GrayGlacier, ForkCondition::Block(0)),
            (
                Hardfork::Paris,
                ForkCondition::TTD { fork_block: Some(0), total_difficulty: U256::from(0) },
            ),
            (Hardfork::Bedrock, ForkCondition::Block(4061224)),
            (Hardfork::Regolith, ForkCondition::Timestamp(1679079600)),
            (Hardfork::Shanghai, ForkCondition::Timestamp(1699981200)),
            (Hardfork::Canyon, ForkCondition::Timestamp(1699981200)),
        ]),
        base_fee_params: BaseFeeParamsKind::Variable(
            vec![
                (Hardfork::London, BaseFeeParams::optimism_goerli()),
                (Hardfork::Canyon, BaseFeeParams::optimism_goerli_canyon()),
            ]
            .into(),
        ),
        prune_delete_limit: 1700,
        snapshot_block_interval: 1_000_000,
        ..Default::default()
    }
    .into()
});

/// The Base Goerli spec
#[cfg(feature = "optimism")]
pub static BASE_GOERLI: Lazy<Arc<ChainSpec>> = Lazy::new(|| {
    ChainSpec {
        chain: Chain::base_goerli(),
        genesis: serde_json::from_str(include_str!("../../res/genesis/goerli_base.json"))
            .expect("Can't deserialize Base Goerli genesis json"),
        genesis_hash: Some(b256!(
            "a3ab140f15ea7f7443a4702da64c10314eb04d488e72974e02e2d728096b4f76"
        )),
        fork_timestamps: ForkTimestamps::default().shanghai(1699981200).canyon(1699981200),
        paris_block_and_final_difficulty: Some((0, U256::from(0))),
        hardforks: BTreeMap::from([
            (Hardfork::Frontier, ForkCondition::Block(0)),
            (Hardfork::Homestead, ForkCondition::Block(0)),
            (Hardfork::Tangerine, ForkCondition::Block(0)),
            (Hardfork::SpuriousDragon, ForkCondition::Block(0)),
            (Hardfork::Byzantium, ForkCondition::Block(0)),
            (Hardfork::Constantinople, ForkCondition::Block(0)),
            (Hardfork::Petersburg, ForkCondition::Block(0)),
            (Hardfork::Istanbul, ForkCondition::Block(0)),
            (Hardfork::MuirGlacier, ForkCondition::Block(0)),
            (Hardfork::Berlin, ForkCondition::Block(0)),
            (Hardfork::London, ForkCondition::Block(0)),
            (Hardfork::ArrowGlacier, ForkCondition::Block(0)),
            (Hardfork::GrayGlacier, ForkCondition::Block(0)),
            (
                Hardfork::Paris,
                ForkCondition::TTD { fork_block: Some(0), total_difficulty: U256::from(0) },
            ),
            (Hardfork::Bedrock, ForkCondition::Block(0)),
            (Hardfork::Regolith, ForkCondition::Timestamp(1683219600)),
            (Hardfork::Shanghai, ForkCondition::Timestamp(1699981200)),
            (Hardfork::Canyon, ForkCondition::Timestamp(1699981200)),
        ]),
        base_fee_params: BaseFeeParamsKind::Variable(
            vec![
                (Hardfork::London, BaseFeeParams::optimism_goerli()),
                (Hardfork::Canyon, BaseFeeParams::optimism_goerli_canyon()),
            ]
            .into(),
        ),
        prune_delete_limit: 1700,
        snapshot_block_interval: 1_000_000,
        ..Default::default()
    }
    .into()
});

/// The Base mainnet spec
#[cfg(feature = "optimism")]
pub static BASE_MAINNET: Lazy<Arc<ChainSpec>> = Lazy::new(|| {
    ChainSpec {
        chain: Chain::base_mainnet(),
        genesis: serde_json::from_str(include_str!("../../res/genesis/base.json"))
            .expect("Can't deserialize Base genesis json"),
        genesis_hash: Some(b256!(
            "f712aa9241cc24369b143cf6dce85f0902a9731e70d66818a3a5845b296c73dd"
        )),
        fork_timestamps: ForkTimestamps::default(),
        paris_block_and_final_difficulty: Some((0, U256::from(0))),
        hardforks: BTreeMap::from([
            (Hardfork::Frontier, ForkCondition::Block(0)),
            (Hardfork::Homestead, ForkCondition::Block(0)),
            (Hardfork::Tangerine, ForkCondition::Block(0)),
            (Hardfork::SpuriousDragon, ForkCondition::Block(0)),
            (Hardfork::Byzantium, ForkCondition::Block(0)),
            (Hardfork::Constantinople, ForkCondition::Block(0)),
            (Hardfork::Petersburg, ForkCondition::Block(0)),
            (Hardfork::Istanbul, ForkCondition::Block(0)),
            (Hardfork::MuirGlacier, ForkCondition::Block(0)),
            (Hardfork::Berlin, ForkCondition::Block(0)),
            (Hardfork::London, ForkCondition::Block(0)),
            (Hardfork::ArrowGlacier, ForkCondition::Block(0)),
            (Hardfork::GrayGlacier, ForkCondition::Block(0)),
            (
                Hardfork::Paris,
                ForkCondition::TTD { fork_block: Some(0), total_difficulty: U256::from(0) },
            ),
            (Hardfork::Bedrock, ForkCondition::Block(0)),
            (Hardfork::Regolith, ForkCondition::Timestamp(0)),
        ]),
        base_fee_params: BaseFeeParamsKind::Variable(
            vec![
                (Hardfork::London, BaseFeeParams::optimism()),
                (Hardfork::Canyon, BaseFeeParams::optimism_canyon()),
            ]
            .into(),
        ),
        prune_delete_limit: 1700,
        snapshot_block_interval: 1_000_000,
        ..Default::default()
    }
    .into()
});

/// A wrapper around [BaseFeeParams] that allows for specifying constant or dynamic EIP-1559
/// parameters based on the active [Hardfork].
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum BaseFeeParamsKind {
    /// Constant [BaseFeeParams]; used for chains that don't have dynamic EIP-1559 parameters
    Constant(BaseFeeParams),
    /// Variable [BaseFeeParams]; used for chains that have dynamic EIP-1559 parameters like
    /// Optimism
    Variable(ForkBaseFeeParams),
}

impl From<BaseFeeParams> for BaseFeeParamsKind {
    fn from(params: BaseFeeParams) -> Self {
        BaseFeeParamsKind::Constant(params)
    }
}

impl From<ForkBaseFeeParams> for BaseFeeParamsKind {
    fn from(params: ForkBaseFeeParams) -> Self {
        BaseFeeParamsKind::Variable(params)
    }
}

/// A type alias to a vector of tuples of [Hardfork] and [BaseFeeParams], sorted by [Hardfork]
/// activation order. This is used to specify dynamic EIP-1559 parameters for chains like Optimism.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ForkBaseFeeParams(Vec<(Hardfork, BaseFeeParams)>);

impl From<Vec<(Hardfork, BaseFeeParams)>> for ForkBaseFeeParams {
    fn from(params: Vec<(Hardfork, BaseFeeParams)>) -> Self {
        ForkBaseFeeParams(params)
    }
}

/// BaseFeeParams contains the config parameters that control block base fee computation
#[derive(Serialize, Deserialize, Debug, Copy, Clone, PartialEq, Eq)]
pub struct BaseFeeParams {
    /// The base_fee_max_change_denominator from EIP-1559
    pub max_change_denominator: u64,
    /// The elasticity multiplier from EIP-1559
    pub elasticity_multiplier: u64,
}

impl BaseFeeParams {
    /// Get the base fee parameters for Ethereum mainnet
    pub const fn ethereum() -> BaseFeeParams {
        BaseFeeParams {
            max_change_denominator: EIP1559_DEFAULT_BASE_FEE_MAX_CHANGE_DENOMINATOR,
            elasticity_multiplier: EIP1559_DEFAULT_ELASTICITY_MULTIPLIER,
        }
    }

    /// Get the base fee parameters for optimism goerli
    #[cfg(feature = "optimism")]
    pub const fn optimism_goerli() -> BaseFeeParams {
        BaseFeeParams {
            max_change_denominator:
                crate::constants::OP_GOERLI_EIP1559_DEFAULT_BASE_FEE_MAX_CHANGE_DENOMINATOR,
            elasticity_multiplier:
                crate::constants::OP_GOERLI_EIP1559_DEFAULT_ELASTICITY_MULTIPLIER,
        }
    }

    /// Get the base fee parameters for optimism goerli (post Canyon)
    #[cfg(feature = "optimism")]
    pub const fn optimism_goerli_canyon() -> BaseFeeParams {
        BaseFeeParams {
            max_change_denominator:
                crate::constants::OP_GOERLI_EIP1559_BASE_FEE_MAX_CHANGE_DENOMINATOR_CANYON,
            elasticity_multiplier:
                crate::constants::OP_GOERLI_EIP1559_DEFAULT_ELASTICITY_MULTIPLIER,
        }
    }

    /// Get the base fee parameters for optimism mainnet
    #[cfg(feature = "optimism")]
    pub const fn optimism() -> BaseFeeParams {
        BaseFeeParams {
            max_change_denominator:
                crate::constants::OP_MAINNET_EIP1559_DEFAULT_BASE_FEE_MAX_CHANGE_DENOMINATOR,
            elasticity_multiplier:
                crate::constants::OP_MAINNET_EIP1559_DEFAULT_ELASTICITY_MULTIPLIER,
        }
    }

    /// Get the base fee parameters for optimism mainnet (post Canyon)
    #[cfg(feature = "optimism")]
    pub const fn optimism_canyon() -> BaseFeeParams {
        BaseFeeParams {
            max_change_denominator:
                crate::constants::OP_MAINNET_EIP1559_BASE_FEE_MAX_CHANGE_DENOMINATOR_CANYON,
            elasticity_multiplier:
                crate::constants::OP_MAINNET_EIP1559_DEFAULT_ELASTICITY_MULTIPLIER,
        }
    }
}

/// An Ethereum chain specification.
///
/// A chain specification describes:
///
/// - Meta-information about the chain (the chain ID)
/// - The genesis block of the chain ([`Genesis`])
/// - What hardforks are activated, and under which conditions
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ChainSpec {
    /// The chain ID
    pub chain: Chain,

    /// The hash of the genesis block.
    ///
    /// This acts as a small cache for known chains. If the chain is known, then the genesis hash
    /// is also known ahead of time, and this will be `Some`.
    #[serde(skip, default)]
    pub genesis_hash: Option<B256>,

    /// The genesis block
    pub genesis: Genesis,

    /// The block at which [Hardfork::Paris] was activated and the final difficulty at this block.
    #[serde(skip, default)]
    pub paris_block_and_final_difficulty: Option<(u64, U256)>,

    /// Timestamps of various hardforks
    ///
    /// This caches entries in `hardforks` map
    #[serde(skip, default)]
    pub fork_timestamps: ForkTimestamps,

    /// The active hard forks and their activation conditions
    pub hardforks: BTreeMap<Hardfork, ForkCondition>,

    /// The deposit contract deployed for PoS
    #[serde(skip, default)]
    pub deposit_contract: Option<DepositContract>,

    /// The parameters that configure how a block's base fee is computed
    pub base_fee_params: BaseFeeParamsKind,

    /// The delete limit for pruner, per block. In the actual pruner run it will be multiplied by
    /// the amount of blocks between pruner runs to account for the difference in amount of new
    /// data coming in.
    #[serde(default)]
    pub prune_delete_limit: usize,

    /// The block interval for creating snapshots. Each snapshot will have that much blocks in it.
    pub snapshot_block_interval: u64,
}

impl Default for ChainSpec {
    fn default() -> ChainSpec {
        ChainSpec {
            chain: Default::default(),
            genesis_hash: Default::default(),
            genesis: Default::default(),
            paris_block_and_final_difficulty: Default::default(),
            fork_timestamps: Default::default(),
            hardforks: Default::default(),
            deposit_contract: Default::default(),
            base_fee_params: BaseFeeParamsKind::Constant(BaseFeeParams::ethereum()),
            prune_delete_limit: MAINNET.prune_delete_limit,
            snapshot_block_interval: Default::default(),
        }
    }
}

impl ChainSpec {
    /// Get information about the chain itself
    pub fn chain(&self) -> Chain {
        self.chain
    }

    /// Returns `true` if this chain contains Optimism configuration.
    #[inline]
    pub fn is_optimism(&self) -> bool {
        self.chain.is_optimism()
    }

    /// Get the genesis block specification.
    ///
    /// To get the header for the genesis block, use [`Self::genesis_header`] instead.
    pub fn genesis(&self) -> &Genesis {
        &self.genesis
    }

    /// Get the header for the genesis block.
    pub fn genesis_header(&self) -> Header {
        // If London is activated at genesis, we set the initial base fee as per EIP-1559.
        let base_fee_per_gas = self.initial_base_fee();

        // If shanghai is activated, initialize the header with an empty withdrawals hash, and
        // empty withdrawals list.
        let withdrawals_root =
            (self.fork(Hardfork::Shanghai).active_at_timestamp(self.genesis.timestamp))
                .then_some(EMPTY_WITHDRAWALS);

        // If Cancun is activated at genesis, we set:
        // * parent beacon block root to 0x0
        // * blob gas used to provided genesis or 0x0
        // * excess blob gas to provided genesis or 0x0
        let (parent_beacon_block_root, blob_gas_used, excess_blob_gas) =
            if self.fork(Hardfork::Cancun).active_at_timestamp(self.genesis.timestamp) {
                let blob_gas_used = self.genesis.blob_gas_used.unwrap_or(0);
                let excess_blob_gas = self.genesis.excess_blob_gas.unwrap_or(0);
                (Some(B256::ZERO), Some(blob_gas_used), Some(excess_blob_gas))
            } else {
                (None, None, None)
            };

        Header {
            parent_hash: B256::ZERO,
            number: 0,
            transactions_root: EMPTY_TRANSACTIONS,
            ommers_hash: EMPTY_OMMER_ROOT_HASH,
            receipts_root: EMPTY_RECEIPTS,
            logs_bloom: Default::default(),
            gas_limit: self.genesis.gas_limit,
            difficulty: self.genesis.difficulty,
            nonce: self.genesis.nonce,
            extra_data: self.genesis.extra_data.clone(),
            state_root: state_root_ref_unhashed(&self.genesis.alloc),
            timestamp: self.genesis.timestamp,
            mix_hash: self.genesis.mix_hash,
            beneficiary: self.genesis.coinbase,
            gas_used: Default::default(),
            base_fee_per_gas,
            withdrawals_root,
            parent_beacon_block_root,
            blob_gas_used,
            excess_blob_gas,
        }
    }

    /// Get the sealed header for the genesis block.
    pub fn sealed_genesis_header(&self) -> SealedHeader {
        SealedHeader { header: self.genesis_header(), hash: self.genesis_hash() }
    }

    /// Get the initial base fee of the genesis block.
    pub fn initial_base_fee(&self) -> Option<u64> {
        // If the base fee is set in the genesis block, we use that instead of the default.
        let genesis_base_fee = self.genesis.base_fee_per_gas.unwrap_or(EIP1559_INITIAL_BASE_FEE);

        // If London is activated at genesis, we set the initial base fee as per EIP-1559.
        (self.fork(Hardfork::London).active_at_block(0)).then_some(genesis_base_fee)
    }

    /// Get the [BaseFeeParams] for the chain at the given timestamp.
    pub fn base_fee_params(&self, timestamp: u64) -> BaseFeeParams {
        match self.base_fee_params {
            BaseFeeParamsKind::Constant(bf_params) => bf_params,
            BaseFeeParamsKind::Variable(ForkBaseFeeParams { 0: ref bf_params }) => {
                // Walk through the base fee params configuration in reverse order, and return the
                // first one that corresponds to a hardfork that is active at the
                // given timestamp.
                for (fork, params) in bf_params.iter().rev() {
                    if self.is_fork_active_at_timestamp(*fork, timestamp) {
                        return *params
                    }
                }

                bf_params.first().map(|(_, params)| *params).unwrap_or(BaseFeeParams::ethereum())
            }
        }
    }

    /// Get the hash of the genesis block.
    pub fn genesis_hash(&self) -> B256 {
        if let Some(hash) = self.genesis_hash {
            hash
        } else {
            self.genesis_header().hash_slow()
        }
    }

    /// Get the timestamp of the genesis block.
    pub fn genesis_timestamp(&self) -> u64 {
        self.genesis.timestamp
    }

    /// Returns the final total difficulty if the given block number is after the Paris hardfork.
    ///
    /// Note: technically this would also be valid for the block before the paris upgrade, but this
    /// edge case is omitted here.
    #[inline]
    pub fn final_paris_total_difficulty(&self, block_number: u64) -> Option<U256> {
        self.paris_block_and_final_difficulty.and_then(|(activated_at, final_difficulty)| {
            if block_number >= activated_at {
                Some(final_difficulty)
            } else {
                None
            }
        })
    }

    /// Get the fork filter for the given hardfork
    pub fn hardfork_fork_filter(&self, fork: Hardfork) -> Option<ForkFilter> {
        match self.fork(fork) {
            ForkCondition::Never => None,
            _ => Some(self.fork_filter(self.satisfy(self.fork(fork)))),
        }
    }

    /// Returns the forks in this specification and their activation conditions.
    pub fn hardforks(&self) -> &BTreeMap<Hardfork, ForkCondition> {
        &self.hardforks
    }

    /// Get the fork id for the given hardfork.
    #[inline]
    pub fn hardfork_fork_id(&self, fork: Hardfork) -> Option<ForkId> {
        match self.fork(fork) {
            ForkCondition::Never => None,
            _ => Some(self.fork_id(&self.satisfy(self.fork(fork)))),
        }
    }

    /// Convenience method to get the fork id for [Hardfork::Shanghai] from a given chainspec.
    #[inline]
    pub fn shanghai_fork_id(&self) -> Option<ForkId> {
        self.hardfork_fork_id(Hardfork::Shanghai)
    }

    /// Convenience method to get the fork id for [Hardfork::Cancun] from a given chainspec.
    #[inline]
    pub fn cancun_fork_id(&self) -> Option<ForkId> {
        self.hardfork_fork_id(Hardfork::Cancun)
    }

    /// Get the fork condition for the given fork.
    pub fn fork(&self, fork: Hardfork) -> ForkCondition {
        self.hardforks.get(&fork).copied().unwrap_or(ForkCondition::Never)
    }

    /// Get an iterator of all hardforks with their respective activation conditions.
    pub fn forks_iter(&self) -> impl Iterator<Item = (Hardfork, ForkCondition)> + '_ {
        self.hardforks.iter().map(|(f, b)| (*f, *b))
    }

    /// Convenience method to check if a fork is active at a given timestamp.
    #[inline]
    pub fn is_fork_active_at_timestamp(&self, fork: Hardfork, timestamp: u64) -> bool {
        self.fork(fork).active_at_timestamp(timestamp)
    }

    /// Convenience method to check if [Hardfork::Shanghai] is active at a given timestamp.
    #[inline]
    pub fn is_shanghai_active_at_timestamp(&self, timestamp: u64) -> bool {
        self.fork_timestamps
            .shanghai
            .map(|shanghai| timestamp >= shanghai)
            .unwrap_or_else(|| self.is_fork_active_at_timestamp(Hardfork::Shanghai, timestamp))
    }

    /// Convenience method to check if [Hardfork::Cancun] is active at a given timestamp.
    #[inline]
    pub fn is_cancun_active_at_timestamp(&self, timestamp: u64) -> bool {
        self.fork_timestamps
            .cancun
            .map(|cancun| timestamp >= cancun)
            .unwrap_or_else(|| self.is_fork_active_at_timestamp(Hardfork::Cancun, timestamp))
    }

    /// Creates a [`ForkFilter`] for the block described by [Head].
    pub fn fork_filter(&self, head: Head) -> ForkFilter {
        let forks = self.forks_iter().filter_map(|(_, condition)| {
            // We filter out TTD-based forks w/o a pre-known block since those do not show up in the
            // fork filter.
            Some(match condition {
                ForkCondition::Block(block) => ForkFilterKey::Block(block),
                ForkCondition::Timestamp(time) => ForkFilterKey::Time(time),
                ForkCondition::TTD { fork_block: Some(block), .. } => ForkFilterKey::Block(block),
                _ => return None,
            })
        });

        ForkFilter::new(head, self.genesis_hash(), self.genesis_timestamp(), forks)
    }

    /// Compute the [`ForkId`] for the given [`Head`] folowing eip-6122 spec
    pub fn fork_id(&self, head: &Head) -> ForkId {
        let mut forkhash = ForkHash::from(self.genesis_hash());
        let mut current_applied = 0;

        // handle all block forks before handling timestamp based forks. see: https://eips.ethereum.org/EIPS/eip-6122
        for (_, cond) in self.forks_iter() {
            // handle block based forks and the sepolia merge netsplit block edge case (TTD
            // ForkCondition with Some(block))
            if let ForkCondition::Block(block) |
            ForkCondition::TTD { fork_block: Some(block), .. } = cond
            {
                if cond.active_at_head(head) {
                    if block != current_applied {
                        forkhash += block;
                        current_applied = block;
                    }
                } else {
                    // we can return here because this block fork is not active, so we set the
                    // `next` value
                    return ForkId { hash: forkhash, next: block }
                }
            }
        }

        // timestamp are ALWAYS applied after the merge.
        //
        // this filter ensures that no block-based forks are returned
        for timestamp in self.forks_iter().filter_map(|(_, cond)| {
            cond.as_timestamp().filter(|time| time > &self.genesis.timestamp)
        }) {
            let cond = ForkCondition::Timestamp(timestamp);
            if cond.active_at_head(head) {
                if timestamp != current_applied {
                    forkhash += timestamp;
                    current_applied = timestamp;
                }
            } else {
                // can safely return here because we have already handled all block forks and
                // have handled all active timestamp forks, and set the next value to the
                // timestamp that is known but not active yet
                return ForkId { hash: forkhash, next: timestamp }
            }
        }

        ForkId { hash: forkhash, next: 0 }
    }

    /// An internal helper function that returns a head block that satisfies a given Fork condition.
    pub(crate) fn satisfy(&self, cond: ForkCondition) -> Head {
        match cond {
            ForkCondition::Block(number) => Head { number, ..Default::default() },
            ForkCondition::Timestamp(timestamp) => {
                // to satisfy every timestamp ForkCondition, we find the last ForkCondition::Block
                // if one exists, and include its block_num in the returned Head
                if let Some(last_block_num) = self.last_block_fork_before_merge_or_timestamp() {
                    return Head { timestamp, number: last_block_num, ..Default::default() }
                }
                Head { timestamp, ..Default::default() }
            }
            ForkCondition::TTD { total_difficulty, .. } => {
                Head { total_difficulty, ..Default::default() }
            }
            ForkCondition::Never => unreachable!(),
        }
    }

    /// An internal helper function that returns the block number of the last block-based
    /// fork that occurs before any existing TTD (merge)/timestamp based forks.
    ///
    /// Note: this returns None if the ChainSpec is not configured with a TTD/Timestamp fork.
    pub(crate) fn last_block_fork_before_merge_or_timestamp(&self) -> Option<u64> {
        let mut hardforks_iter = self.forks_iter().peekable();
        while let Some((_, curr_cond)) = hardforks_iter.next() {
            if let Some((_, next_cond)) = hardforks_iter.peek() {
                // peek and find the first occurence of ForkCondition::TTD (merge) , or in
                // custom ChainSpecs, the first occurence of
                // ForkCondition::Timestamp. If curr_cond is ForkCondition::Block at
                // this point, which it should be in most "normal" ChainSpecs,
                // return its block_num
                match next_cond {
                    ForkCondition::TTD { fork_block, .. } => {
                        // handle Sepolia merge netsplit case
                        if fork_block.is_some() {
                            return *fork_block
                        }
                        // ensure curr_cond is indeed ForkCondition::Block and return block_num
                        if let ForkCondition::Block(block_num) = curr_cond {
                            return Some(block_num)
                        }
                    }
                    ForkCondition::Timestamp(_) => {
                        // ensure curr_cond is indeed ForkCondition::Block and return block_num
                        if let ForkCondition::Block(block_num) = curr_cond {
                            return Some(block_num)
                        }
                    }
                    ForkCondition::Block(_) | ForkCondition::Never => continue,
                }
            }
        }
        None
    }

    /// Build a chainspec using [`ChainSpecBuilder`]
    pub fn builder() -> ChainSpecBuilder {
        ChainSpecBuilder::default()
    }
}

impl From<Genesis> for ChainSpec {
    fn from(genesis: Genesis) -> Self {
        // Block-based hardforks
        let hardfork_opts = [
            (Hardfork::Homestead, genesis.config.homestead_block),
            (Hardfork::Dao, genesis.config.dao_fork_block),
            (Hardfork::Tangerine, genesis.config.eip150_block),
            (Hardfork::SpuriousDragon, genesis.config.eip155_block),
            (Hardfork::Byzantium, genesis.config.byzantium_block),
            (Hardfork::Constantinople, genesis.config.constantinople_block),
            (Hardfork::Petersburg, genesis.config.petersburg_block),
            (Hardfork::Istanbul, genesis.config.istanbul_block),
            (Hardfork::MuirGlacier, genesis.config.muir_glacier_block),
            (Hardfork::Berlin, genesis.config.berlin_block),
            (Hardfork::London, genesis.config.london_block),
            (Hardfork::ArrowGlacier, genesis.config.arrow_glacier_block),
            (Hardfork::GrayGlacier, genesis.config.gray_glacier_block),
        ];
        let mut hardforks = hardfork_opts
            .iter()
            .filter_map(|(hardfork, opt)| opt.map(|block| (*hardfork, ForkCondition::Block(block))))
            .collect::<BTreeMap<_, _>>();

        // Paris
        if let Some(ttd) = genesis.config.terminal_total_difficulty {
            hardforks.insert(
                Hardfork::Paris,
                ForkCondition::TTD {
                    total_difficulty: ttd,
                    fork_block: genesis.config.merge_netsplit_block,
                },
            );
        }

        // Time-based hardforks
        let time_hardfork_opts = [
            (Hardfork::Shanghai, genesis.config.shanghai_time),
            (Hardfork::Cancun, genesis.config.cancun_time),
        ];

        let time_hardforks = time_hardfork_opts
            .iter()
            .filter_map(|(hardfork, opt)| {
                opt.map(|time| (*hardfork, ForkCondition::Timestamp(time)))
            })
            .collect::<BTreeMap<_, _>>();

        hardforks.extend(time_hardforks);

        Self {
            chain: genesis.config.chain_id.into(),
            genesis,
            genesis_hash: None,
            fork_timestamps: ForkTimestamps::from_hardforks(&hardforks),
            hardforks,
            paris_block_and_final_difficulty: None,
            deposit_contract: None,
            ..Default::default()
        }
    }
}

/// Various timestamps of forks
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct ForkTimestamps {
    /// The timestamp of the shanghai fork
    pub shanghai: Option<u64>,
    /// The timestamp of the cancun fork
    pub cancun: Option<u64>,
    /// The timestamp of the Regolith fork
    #[cfg(feature = "optimism")]
    pub regolith: Option<u64>,
    /// The timestamp of the Canyon fork
    #[cfg(feature = "optimism")]
    pub canyon: Option<u64>,
}

impl ForkTimestamps {
    /// Creates a new [`ForkTimestamps`] from the given hardforks by extracing the timestamps
    fn from_hardforks(forks: &BTreeMap<Hardfork, ForkCondition>) -> Self {
        let mut timestamps = ForkTimestamps::default();
        if let Some(shanghai) = forks.get(&Hardfork::Shanghai).and_then(|f| f.as_timestamp()) {
            timestamps = timestamps.shanghai(shanghai);
        }
        if let Some(cancun) = forks.get(&Hardfork::Cancun).and_then(|f| f.as_timestamp()) {
            timestamps = timestamps.cancun(cancun);
        }
        #[cfg(feature = "optimism")]
        {
            if let Some(regolith) = forks.get(&Hardfork::Regolith).and_then(|f| f.as_timestamp()) {
                timestamps = timestamps.regolith(regolith);
            }
            if let Some(canyon) = forks.get(&Hardfork::Canyon).and_then(|f| f.as_timestamp()) {
                timestamps = timestamps.canyon(canyon);
            }
        }
        timestamps
    }

    /// Sets the given shanghai timestamp
    pub fn shanghai(mut self, shanghai: u64) -> Self {
        self.shanghai = Some(shanghai);
        self
    }

    /// Sets the given cancun timestamp
    pub fn cancun(mut self, cancun: u64) -> Self {
        self.cancun = Some(cancun);
        self
    }

    /// Sets the given regolith timestamp
    #[cfg(feature = "optimism")]
    pub fn regolith(mut self, regolith: u64) -> Self {
        self.regolith = Some(regolith);
        self
    }

    /// Sets the given canyon timestamp
    #[cfg(feature = "optimism")]
    pub fn canyon(mut self, canyon: u64) -> Self {
        self.canyon = Some(canyon);
        self
    }
}

/// A helper type for compatibility with geth's config
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum AllGenesisFormats {
    /// The reth genesis format
    Reth(ChainSpec),
    /// The geth genesis format
    Geth(Genesis),
}

impl From<Genesis> for AllGenesisFormats {
    fn from(genesis: Genesis) -> Self {
        Self::Geth(genesis)
    }
}

impl From<ChainSpec> for AllGenesisFormats {
    fn from(genesis: ChainSpec) -> Self {
        Self::Reth(genesis)
    }
}

impl From<Arc<ChainSpec>> for AllGenesisFormats {
    fn from(genesis: Arc<ChainSpec>) -> Self {
        Arc::try_unwrap(genesis).unwrap_or_else(|arc| (*arc).clone()).into()
    }
}

impl From<AllGenesisFormats> for ChainSpec {
    fn from(genesis: AllGenesisFormats) -> Self {
        match genesis {
            AllGenesisFormats::Geth(genesis) => genesis.into(),
            AllGenesisFormats::Reth(genesis) => genesis,
        }
    }
}

/// A helper to build custom chain specs
#[derive(Debug, Default, Clone)]
pub struct ChainSpecBuilder {
    chain: Option<Chain>,
    genesis: Option<Genesis>,
    hardforks: BTreeMap<Hardfork, ForkCondition>,
}

impl ChainSpecBuilder {
    /// Construct a new builder from the mainnet chain spec.
    pub fn mainnet() -> Self {
        Self {
            chain: Some(MAINNET.chain),
            genesis: Some(MAINNET.genesis.clone()),
            hardforks: MAINNET.hardforks.clone(),
        }
    }

    /// Set the chain ID
    pub fn chain(mut self, chain: Chain) -> Self {
        self.chain = Some(chain);
        self
    }

    /// Set the genesis block.
    pub fn genesis(mut self, genesis: Genesis) -> Self {
        self.genesis = Some(genesis);
        self
    }

    /// Add the given fork with the given activation condition to the spec.
    pub fn with_fork(mut self, fork: Hardfork, condition: ForkCondition) -> Self {
        self.hardforks.insert(fork, condition);
        self
    }

    /// Enable the Paris hardfork at the given TTD.
    ///
    /// Does not set the merge netsplit block.
    pub fn paris_at_ttd(self, ttd: U256) -> Self {
        self.with_fork(
            Hardfork::Paris,
            ForkCondition::TTD { total_difficulty: ttd, fork_block: None },
        )
    }

    /// Enable Frontier at genesis.
    pub fn frontier_activated(mut self) -> Self {
        self.hardforks.insert(Hardfork::Frontier, ForkCondition::Block(0));
        self
    }

    /// Enable Homestead at genesis.
    pub fn homestead_activated(mut self) -> Self {
        self = self.frontier_activated();
        self.hardforks.insert(Hardfork::Homestead, ForkCondition::Block(0));
        self
    }

    /// Enable Tangerine at genesis.
    pub fn tangerine_whistle_activated(mut self) -> Self {
        self = self.homestead_activated();
        self.hardforks.insert(Hardfork::Tangerine, ForkCondition::Block(0));
        self
    }

    /// Enable Spurious Dragon at genesis.
    pub fn spurious_dragon_activated(mut self) -> Self {
        self = self.tangerine_whistle_activated();
        self.hardforks.insert(Hardfork::SpuriousDragon, ForkCondition::Block(0));
        self
    }

    /// Enable Byzantium at genesis.
    pub fn byzantium_activated(mut self) -> Self {
        self = self.spurious_dragon_activated();
        self.hardforks.insert(Hardfork::Byzantium, ForkCondition::Block(0));
        self
    }

    /// Enable Petersburg at genesis.
    pub fn petersburg_activated(mut self) -> Self {
        self = self.byzantium_activated();
        self.hardforks.insert(Hardfork::Petersburg, ForkCondition::Block(0));
        self
    }

    /// Enable Istanbul at genesis.
    pub fn istanbul_activated(mut self) -> Self {
        self = self.petersburg_activated();
        self.hardforks.insert(Hardfork::Istanbul, ForkCondition::Block(0));
        self
    }

    /// Enable Berlin at genesis.
    pub fn berlin_activated(mut self) -> Self {
        self = self.istanbul_activated();
        self.hardforks.insert(Hardfork::Berlin, ForkCondition::Block(0));
        self
    }

    /// Enable London at genesis.
    pub fn london_activated(mut self) -> Self {
        self = self.berlin_activated();
        self.hardforks.insert(Hardfork::London, ForkCondition::Block(0));
        self
    }

    /// Enable Paris at genesis.
    pub fn paris_activated(mut self) -> Self {
        self = self.london_activated();
        self.hardforks.insert(
            Hardfork::Paris,
            ForkCondition::TTD { fork_block: Some(0), total_difficulty: U256::ZERO },
        );
        self
    }

    /// Enable Shanghai at genesis.
    pub fn shanghai_activated(mut self) -> Self {
        self = self.paris_activated();
        self.hardforks.insert(Hardfork::Shanghai, ForkCondition::Timestamp(0));
        self
    }

    /// Enable Cancun at genesis.
    pub fn cancun_activated(mut self) -> Self {
        self = self.shanghai_activated();
        self.hardforks.insert(Hardfork::Cancun, ForkCondition::Timestamp(0));
        self
    }

    /// Enable Bedrock at genesis
    #[cfg(feature = "optimism")]
    pub fn bedrock_activated(mut self) -> Self {
        self = self.paris_activated();
        self.hardforks.insert(Hardfork::Bedrock, ForkCondition::Block(0));
        self
    }

    /// Enable Regolith at genesis
    #[cfg(feature = "optimism")]
    pub fn regolith_activated(mut self) -> Self {
        self = self.bedrock_activated();
        self.hardforks.insert(Hardfork::Regolith, ForkCondition::Timestamp(0));
        self
    }

    /// Enable Canyon at genesis
    #[cfg(feature = "optimism")]
    pub fn canyon_activated(mut self) -> Self {
        self = self.regolith_activated();
        // Canyon also activates changes from L1's Shanghai hardfork
        self.hardforks.insert(Hardfork::Shanghai, ForkCondition::Timestamp(0));
        self.hardforks.insert(Hardfork::Canyon, ForkCondition::Timestamp(0));
        self
    }

    /// Build the resulting [`ChainSpec`].
    ///
    /// # Panics
    ///
    /// This function panics if the chain ID and genesis is not set ([`Self::chain`] and
    /// [`Self::genesis`])
    pub fn build(self) -> ChainSpec {
        ChainSpec {
            chain: self.chain.expect("The chain is required"),
            genesis: self.genesis.expect("The genesis is required"),
            genesis_hash: None,
            fork_timestamps: ForkTimestamps::from_hardforks(&self.hardforks),
            hardforks: self.hardforks,
            paris_block_and_final_difficulty: None,
            deposit_contract: None,
            ..Default::default()
        }
    }
}

impl From<&Arc<ChainSpec>> for ChainSpecBuilder {
    fn from(value: &Arc<ChainSpec>) -> Self {
        Self {
            chain: Some(value.chain),
            genesis: Some(value.genesis.clone()),
            hardforks: value.hardforks.clone(),
        }
    }
}

/// The condition at which a fork is activated.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ForkCondition {
    /// The fork is activated after a certain block.
    Block(BlockNumber),
    /// The fork is activated after a total difficulty has been reached.
    TTD {
        /// The block number at which TTD is reached, if it is known.
        ///
        /// This should **NOT** be set unless you want this block advertised as [EIP-2124][eip2124]
        /// `FORK_NEXT`. This is currently only the case for Sepolia and Holesky.
        ///
        /// [eip2124]: https://eips.ethereum.org/EIPS/eip-2124
        fork_block: Option<BlockNumber>,
        /// The total difficulty after which the fork is activated.
        total_difficulty: U256,
    },
    /// The fork is activated after a specific timestamp.
    Timestamp(u64),
    /// The fork is never activated
    #[default]
    Never,
}

impl ForkCondition {
    /// Returns true if the fork condition is timestamp based.
    pub fn is_timestamp(&self) -> bool {
        matches!(self, ForkCondition::Timestamp(_))
    }

    /// Checks whether the fork condition is satisfied at the given block.
    ///
    /// For TTD conditions, this will only return true if the activation block is already known.
    ///
    /// For timestamp conditions, this will always return false.
    pub fn active_at_block(&self, current_block: BlockNumber) -> bool {
        match self {
            ForkCondition::Block(block) => current_block >= *block,
            ForkCondition::TTD { fork_block: Some(block), .. } => current_block >= *block,
            _ => false,
        }
    }

    /// Checks if the given block is the first block that satisfies the fork condition.
    ///
    /// This will return false for any condition that is not block based.
    pub fn transitions_at_block(&self, current_block: BlockNumber) -> bool {
        match self {
            ForkCondition::Block(block) => current_block == *block,
            _ => false,
        }
    }

    /// Checks whether the fork condition is satisfied at the given total difficulty and difficulty
    /// of a current block.
    ///
    /// The fork is considered active if the _previous_ total difficulty is above the threshold.
    /// To achieve that, we subtract the passed `difficulty` from the current block's total
    /// difficulty, and check if it's above the Fork Condition's total difficulty (here:
    /// 58_750_000_000_000_000_000_000)
    ///
    /// This will return false for any condition that is not TTD-based.
    pub fn active_at_ttd(&self, ttd: U256, difficulty: U256) -> bool {
        if let ForkCondition::TTD { total_difficulty, .. } = self {
            ttd.saturating_sub(difficulty) >= *total_difficulty
        } else {
            false
        }
    }

    /// Checks whether the fork condition is satisfied at the given timestamp.
    ///
    /// This will return false for any condition that is not timestamp-based.
    pub fn active_at_timestamp(&self, timestamp: u64) -> bool {
        if let ForkCondition::Timestamp(time) = self {
            timestamp >= *time
        } else {
            false
        }
    }

    /// Checks whether the fork condition is satisfied at the given head block.
    ///
    /// This will return true if:
    ///
    /// - The condition is satisfied by the block number;
    /// - The condition is satisfied by the timestamp;
    /// - or the condition is satisfied by the total difficulty
    pub fn active_at_head(&self, head: &Head) -> bool {
        self.active_at_block(head.number) ||
            self.active_at_timestamp(head.timestamp) ||
            self.active_at_ttd(head.total_difficulty, head.difficulty)
    }

    /// Get the total terminal difficulty for this fork condition.
    ///
    /// Returns `None` for fork conditions that are not TTD based.
    pub fn ttd(&self) -> Option<U256> {
        match self {
            ForkCondition::TTD { total_difficulty, .. } => Some(*total_difficulty),
            _ => None,
        }
    }

    /// Returns the timestamp of the fork condition, if it is timestamp based.
    pub fn as_timestamp(&self) -> Option<u64> {
        match self {
            ForkCondition::Timestamp(timestamp) => Some(*timestamp),
            _ => None,
        }
    }
}

/// A container to pretty-print a hardfork.
///
/// The fork is formatted depending on its fork condition:
///
/// - Block and timestamp based forks are formatted in the same manner (`{name} <({eip})>
///   @{condition}`)
/// - TTD based forks are formatted separately as `{name} <({eip})> @{ttd} (network is <not> known
///   to be merged)`
///
/// An optional EIP can be attached to the fork to display as well. This should generally be in the
/// form of just `EIP-x`, e.g. `EIP-1559`.
#[derive(Debug)]
struct DisplayFork {
    /// The name of the hardfork (e.g. Frontier)
    name: String,
    /// The fork condition
    activated_at: ForkCondition,
    /// An optional EIP (e.g. `EIP-1559`).
    eip: Option<String>,
}

impl Display for DisplayFork {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let name_with_eip = if let Some(eip) = &self.eip {
            format!("{} ({})", self.name, eip)
        } else {
            self.name.clone()
        };

        match self.activated_at {
            ForkCondition::Block(at) | ForkCondition::Timestamp(at) => {
                write!(f, "{:32} @{}", name_with_eip, at)?;
            }
            ForkCondition::TTD { fork_block, total_difficulty } => {
                writeln!(
                    f,
                    "{:32} @{} ({})",
                    name_with_eip,
                    total_difficulty,
                    if fork_block.is_some() {
                        "network is known to be merged"
                    } else {
                        "network is not known to be merged"
                    }
                )?;
            }
            ForkCondition::Never => unreachable!(),
        }

        Ok(())
    }
}

/// A container for pretty-printing a list of hardforks.
///
/// # Examples
///
/// ```
/// # use reth_primitives::MAINNET;
/// # use reth_primitives::DisplayHardforks;
/// println!("{}", DisplayHardforks::new(MAINNET.hardforks()));
/// ```
///
/// An example of the output:
///
/// ```text
/// Pre-merge hard forks (block based):
// - Frontier                         @0
// - Homestead                        @1150000
// - Dao                              @1920000
// - Tangerine                        @2463000
// - SpuriousDragon                   @2675000
// - Byzantium                        @4370000
// - Constantinople                   @7280000
// - Petersburg                       @7280000
// - Istanbul                         @9069000
// - MuirGlacier                      @9200000
// - Berlin                           @12244000
// - London                           @12965000
// - ArrowGlacier                     @13773000
// - GrayGlacier                      @15050000
// Merge hard forks:
// - Paris                            @58750000000000000000000 (network is not known to be merged)
//
// Post-merge hard forks (timestamp based):
// - Shanghai                         @1681338455
/// ```
#[derive(Debug)]
pub struct DisplayHardforks {
    /// A list of pre-merge (block based) hardforks
    pre_merge: Vec<DisplayFork>,
    /// A list of merge (TTD based) hardforks
    with_merge: Vec<DisplayFork>,
    /// A list of post-merge (timestamp based) hardforks
    post_merge: Vec<DisplayFork>,
}

impl Display for DisplayHardforks {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Pre-merge hard forks (block based):")?;
        for fork in self.pre_merge.iter() {
            writeln!(f, "- {fork}")?;
        }

        if !self.with_merge.is_empty() {
            writeln!(f, "Merge hard forks:")?;
            for fork in self.with_merge.iter() {
                writeln!(f, "- {fork}")?;
            }
        }

        if !self.post_merge.is_empty() {
            writeln!(f, "Post-merge hard forks (timestamp based):")?;
            for fork in self.post_merge.iter() {
                writeln!(f, "- {fork}")?;
            }
        }

        Ok(())
    }
}

impl DisplayHardforks {
    /// Creates a new [`DisplayHardforks`] from an iterator of hardforks.
    pub fn new(hardforks: &BTreeMap<Hardfork, ForkCondition>) -> Self {
        Self::from_iter(hardforks.iter())
    }
}

impl<'a, 'b> FromIterator<(&'a Hardfork, &'b ForkCondition)> for DisplayHardforks {
    fn from_iter<T: IntoIterator<Item = (&'a Hardfork, &'b ForkCondition)>>(iter: T) -> Self {
        let mut pre_merge = Vec::new();
        let mut with_merge = Vec::new();
        let mut post_merge = Vec::new();

        for (fork, condition) in iter {
            let display_fork =
                DisplayFork { name: fork.to_string(), activated_at: *condition, eip: None };

            match condition {
                ForkCondition::Block(_) => {
                    pre_merge.push(display_fork);
                }
                ForkCondition::TTD { .. } => {
                    with_merge.push(display_fork);
                }
                ForkCondition::Timestamp(_) => {
                    post_merge.push(display_fork);
                }
                ForkCondition::Never => continue,
            }
        }

        Self { pre_merge, with_merge, post_merge }
    }
}

/// PoS deposit contract details.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepositContract {
    /// Deposit Contract Address
    pub address: Address,
    /// Deployment Block
    pub block: BlockNumber,
    /// `DepositEvent` event signature
    pub topic: B256,
}

impl DepositContract {
    fn new(address: Address, block: BlockNumber, topic: B256) -> Self {
        DepositContract { address, block, topic }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        b256, hex, trie::TrieAccount, ChainConfig, GenesisAccount, NamedChain, B256, DEV, GOERLI,
        HOLESKY, MAINNET, SEPOLIA, U256,
    };
    use alloy_rlp::Encodable;
    use bytes::BytesMut;
    use std::{collections::HashMap, str::FromStr};

    #[cfg(feature = "optimism")]
    use crate::OP_GOERLI;

    fn test_fork_ids(spec: &ChainSpec, cases: &[(Head, ForkId)]) {
        for (block, expected_id) in cases {
            let computed_id = spec.fork_id(block);
            assert_eq!(
                expected_id, &computed_id,
                "Expected fork ID {:?}, computed fork ID {:?} at block {}",
                expected_id, computed_id, block.number
            );
        }
    }

    fn test_hardfork_fork_ids(spec: &ChainSpec, cases: &[(Hardfork, ForkId)]) {
        for (hardfork, expected_id) in cases {
            if let Some(computed_id) = spec.hardfork_fork_id(*hardfork) {
                assert_eq!(
                    expected_id, &computed_id,
                    "Expected fork ID {:?}, computed fork ID {:?} for hardfork {}",
                    expected_id, computed_id, hardfork
                );
                if let Hardfork::Shanghai = hardfork {
                    if let Some(shangai_id) = spec.shanghai_fork_id() {
                        assert_eq!(
                            expected_id, &shangai_id,
                            "Expected fork ID {:?}, computed fork ID {:?} for Shanghai hardfork",
                            expected_id, computed_id
                        );
                    } else {
                        panic!("Expected ForkCondition to return Some for Hardfork::Shanghai");
                    }
                }
            }
        }
    }

    #[test]
    fn test_hardfork_list_display_mainnet() {
        assert_eq!(
            DisplayHardforks::new(MAINNET.hardforks()).to_string(),
            "Pre-merge hard forks (block based):
- Frontier                         @0
- Homestead                        @1150000
- Dao                              @1920000
- Tangerine                        @2463000
- SpuriousDragon                   @2675000
- Byzantium                        @4370000
- Constantinople                   @7280000
- Petersburg                       @7280000
- Istanbul                         @9069000
- MuirGlacier                      @9200000
- Berlin                           @12244000
- London                           @12965000
- ArrowGlacier                     @13773000
- GrayGlacier                      @15050000
Merge hard forks:
- Paris                            @58750000000000000000000 (network is not known to be merged)

Post-merge hard forks (timestamp based):
- Shanghai                         @1681338455
"
        );
    }

    #[test]
    fn test_hardfork_list_ignores_disabled_forks() {
        let spec = ChainSpec::builder()
            .chain(Chain::mainnet())
            .genesis(Genesis::default())
            .with_fork(Hardfork::Frontier, ForkCondition::Block(0))
            .with_fork(Hardfork::Shanghai, ForkCondition::Never)
            .build();
        assert_eq!(
            DisplayHardforks::new(spec.hardforks()).to_string(),
            "Pre-merge hard forks (block based):
- Frontier                         @0
"
        );
    }

    // Tests that the ForkTimestamps are correctly set up.
    #[test]
    fn test_fork_timestamps() {
        let spec = ChainSpec::builder().chain(Chain::mainnet()).genesis(Genesis::default()).build();
        assert!(spec.fork_timestamps.shanghai.is_none());

        let spec = ChainSpec::builder()
            .chain(Chain::mainnet())
            .genesis(Genesis::default())
            .with_fork(Hardfork::Shanghai, ForkCondition::Timestamp(1337))
            .build();
        assert_eq!(spec.fork_timestamps.shanghai, Some(1337));
        assert!(spec.is_shanghai_active_at_timestamp(1337));
        assert!(!spec.is_shanghai_active_at_timestamp(1336));
    }

    // Tests that all predefined timestamps are correctly set up in the chainspecs
    #[test]
    fn test_predefined_chain_spec_fork_timestamps() {
        fn ensure_timestamp_fork_conditions(spec: &ChainSpec) {
            // This is a sanity test that ensures we always set all currently known fork timestamps,
            // this will fail if a new timestamp based fork condition has added to the hardforks but
            // no corresponding entry in the ForkTimestamp types, See also
            // [ForkTimestamps::from_hardforks]

            // currently there are only 1 timestamps known: shanghai
            let known_timestamp_based_forks = 1;
            let num_timestamp_based_forks =
                spec.hardforks.values().copied().filter(ForkCondition::is_timestamp).count();
            assert_eq!(num_timestamp_based_forks, known_timestamp_based_forks);

            // ensures all timestamp forks are set
            assert!(spec.fork_timestamps.shanghai.is_some());
        }

        for spec in [&*MAINNET, &*SEPOLIA] {
            ensure_timestamp_fork_conditions(spec);
        }
    }

    // Tests that we skip any fork blocks in block #0 (the genesis ruleset)
    #[test]
    fn ignores_genesis_fork_blocks() {
        let spec = ChainSpec::builder()
            .chain(Chain::mainnet())
            .genesis(Genesis::default())
            .with_fork(Hardfork::Frontier, ForkCondition::Block(0))
            .with_fork(Hardfork::Homestead, ForkCondition::Block(0))
            .with_fork(Hardfork::Tangerine, ForkCondition::Block(0))
            .with_fork(Hardfork::SpuriousDragon, ForkCondition::Block(0))
            .with_fork(Hardfork::Byzantium, ForkCondition::Block(0))
            .with_fork(Hardfork::Constantinople, ForkCondition::Block(0))
            .with_fork(Hardfork::Istanbul, ForkCondition::Block(0))
            .with_fork(Hardfork::MuirGlacier, ForkCondition::Block(0))
            .with_fork(Hardfork::Berlin, ForkCondition::Block(0))
            .with_fork(Hardfork::London, ForkCondition::Block(0))
            .with_fork(Hardfork::ArrowGlacier, ForkCondition::Block(0))
            .with_fork(Hardfork::GrayGlacier, ForkCondition::Block(0))
            .build();

        assert_eq!(spec.hardforks().len(), 12, "12 forks should be active.");
        assert_eq!(
            spec.fork_id(&Head { number: 1, ..Default::default() }),
            ForkId { hash: ForkHash::from(spec.genesis_hash()), next: 0 },
            "the fork ID should be the genesis hash; forks at genesis are ignored for fork filters"
        );
    }

    #[test]
    fn ignores_duplicate_fork_blocks() {
        let empty_genesis = Genesis::default();
        let unique_spec = ChainSpec::builder()
            .chain(Chain::mainnet())
            .genesis(empty_genesis.clone())
            .with_fork(Hardfork::Frontier, ForkCondition::Block(0))
            .with_fork(Hardfork::Homestead, ForkCondition::Block(1))
            .build();

        let duplicate_spec = ChainSpec::builder()
            .chain(Chain::mainnet())
            .genesis(empty_genesis)
            .with_fork(Hardfork::Frontier, ForkCondition::Block(0))
            .with_fork(Hardfork::Homestead, ForkCondition::Block(1))
            .with_fork(Hardfork::Tangerine, ForkCondition::Block(1))
            .build();

        assert_eq!(
            unique_spec.fork_id(&Head { number: 2, ..Default::default() }),
            duplicate_spec.fork_id(&Head { number: 2, ..Default::default() }),
            "duplicate fork blocks should be deduplicated for fork filters"
        );
    }

    #[test]
    fn test_chainspec_satisfy() {
        let empty_genesis = Genesis::default();
        // happy path test case
        let happy_path_case = ChainSpec::builder()
            .chain(Chain::mainnet())
            .genesis(empty_genesis.clone())
            .with_fork(Hardfork::Frontier, ForkCondition::Block(0))
            .with_fork(Hardfork::Homestead, ForkCondition::Block(73))
            .with_fork(Hardfork::Shanghai, ForkCondition::Timestamp(11313123))
            .build();
        let happy_path_head = happy_path_case.satisfy(ForkCondition::Timestamp(11313123));
        let happy_path_expected = Head { number: 73, timestamp: 11313123, ..Default::default() };
        assert_eq!(
            happy_path_head, happy_path_expected,
            "expected satisfy() to return {:#?}, but got {:#?} ",
            happy_path_expected, happy_path_head
        );
        // multiple timestamp test case (i.e Shanghai -> Cancun)
        let multiple_timestamp_fork_case = ChainSpec::builder()
            .chain(Chain::mainnet())
            .genesis(empty_genesis.clone())
            .with_fork(Hardfork::Frontier, ForkCondition::Block(0))
            .with_fork(Hardfork::Homestead, ForkCondition::Block(73))
            .with_fork(Hardfork::Shanghai, ForkCondition::Timestamp(11313123))
            .with_fork(Hardfork::Cancun, ForkCondition::Timestamp(11313398))
            .build();
        let multi_timestamp_head =
            multiple_timestamp_fork_case.satisfy(ForkCondition::Timestamp(11313398));
        let mult_timestamp_expected =
            Head { number: 73, timestamp: 11313398, ..Default::default() };
        assert_eq!(
            multi_timestamp_head, mult_timestamp_expected,
            "expected satisfy() to return {:#?}, but got {:#?} ",
            mult_timestamp_expected, multi_timestamp_head
        );
        // no ForkCondition::Block test case
        let no_block_fork_case = ChainSpec::builder()
            .chain(Chain::mainnet())
            .genesis(empty_genesis.clone())
            .with_fork(Hardfork::Shanghai, ForkCondition::Timestamp(11313123))
            .build();
        let no_block_fork_head = no_block_fork_case.satisfy(ForkCondition::Timestamp(11313123));
        let no_block_fork_expected = Head { number: 0, timestamp: 11313123, ..Default::default() };
        assert_eq!(
            no_block_fork_head, no_block_fork_expected,
            "expected satisfy() to return {:#?}, but got {:#?} ",
            no_block_fork_expected, no_block_fork_head
        );
        // spec w/ ForkCondition::TTD with block_num test case (Sepolia merge netsplit edge case)
        let fork_cond_ttd_blocknum_case = ChainSpec::builder()
            .chain(Chain::mainnet())
            .genesis(empty_genesis.clone())
            .with_fork(Hardfork::Frontier, ForkCondition::Block(0))
            .with_fork(Hardfork::Homestead, ForkCondition::Block(73))
            .with_fork(
                Hardfork::Paris,
                ForkCondition::TTD {
                    fork_block: Some(101),
                    total_difficulty: U256::from(10_790_000),
                },
            )
            .with_fork(Hardfork::Shanghai, ForkCondition::Timestamp(11313123))
            .build();
        let fork_cond_ttd_blocknum_head =
            fork_cond_ttd_blocknum_case.satisfy(ForkCondition::Timestamp(11313123));
        let fork_cond_ttd_blocknum_expected =
            Head { number: 101, timestamp: 11313123, ..Default::default() };
        assert_eq!(
            fork_cond_ttd_blocknum_head, fork_cond_ttd_blocknum_expected,
            "expected satisfy() to return {:#?}, but got {:#?} ",
            fork_cond_ttd_blocknum_expected, fork_cond_ttd_blocknum_head
        );

        // spec w/ only ForkCondition::Block - test the match arm for ForkCondition::Block to ensure
        // no regressions, for these ForkConditions(Block/TTD) - a separate chain spec definition is
        // technically unecessary - but we include it here for thoroughness
        let fork_cond_block_only_case = ChainSpec::builder()
            .chain(Chain::mainnet())
            .genesis(empty_genesis)
            .with_fork(Hardfork::Frontier, ForkCondition::Block(0))
            .with_fork(Hardfork::Homestead, ForkCondition::Block(73))
            .build();
        let fork_cond_block_only_head = fork_cond_block_only_case.satisfy(ForkCondition::Block(73));
        let fork_cond_block_only_expected = Head { number: 73, ..Default::default() };
        assert_eq!(
            fork_cond_block_only_head, fork_cond_block_only_expected,
            "expected satisfy() to return {:#?}, but got {:#?} ",
            fork_cond_block_only_expected, fork_cond_block_only_head
        );
        // Fork::ConditionTTD test case without a new chain spec to demonstrate ChainSpec::satisfy
        // is independent of ChainSpec for this(these - including ForkCondition::Block) match arm(s)
        let fork_cond_ttd_no_new_spec = fork_cond_block_only_case.satisfy(ForkCondition::TTD {
            fork_block: None,
            total_difficulty: U256::from(10_790_000),
        });
        let fork_cond_ttd_no_new_spec_expected =
            Head { total_difficulty: U256::from(10_790_000), ..Default::default() };
        assert_eq!(
            fork_cond_ttd_no_new_spec, fork_cond_ttd_no_new_spec_expected,
            "expected satisfy() to return {:#?}, but got {:#?} ",
            fork_cond_ttd_no_new_spec_expected, fork_cond_ttd_no_new_spec
        );
    }

    #[test]
    fn mainnet_hardfork_fork_ids() {
        test_hardfork_fork_ids(
            &MAINNET,
            &[
                (
                    Hardfork::Frontier,
                    ForkId { hash: ForkHash([0xfc, 0x64, 0xec, 0x04]), next: 1150000 },
                ),
                (
                    Hardfork::Homestead,
                    ForkId { hash: ForkHash([0x97, 0xc2, 0xc3, 0x4c]), next: 1920000 },
                ),
                (Hardfork::Dao, ForkId { hash: ForkHash([0x91, 0xd1, 0xf9, 0x48]), next: 2463000 }),
                (
                    Hardfork::Tangerine,
                    ForkId { hash: ForkHash([0x7a, 0x64, 0xda, 0x13]), next: 2675000 },
                ),
                (
                    Hardfork::SpuriousDragon,
                    ForkId { hash: ForkHash([0x3e, 0xdd, 0x5b, 0x10]), next: 4370000 },
                ),
                (
                    Hardfork::Byzantium,
                    ForkId { hash: ForkHash([0xa0, 0x0b, 0xc3, 0x24]), next: 7280000 },
                ),
                (
                    Hardfork::Constantinople,
                    ForkId { hash: ForkHash([0x66, 0x8d, 0xb0, 0xaf]), next: 9069000 },
                ),
                (
                    Hardfork::Petersburg,
                    ForkId { hash: ForkHash([0x66, 0x8d, 0xb0, 0xaf]), next: 9069000 },
                ),
                (
                    Hardfork::Istanbul,
                    ForkId { hash: ForkHash([0x87, 0x9d, 0x6e, 0x30]), next: 9200000 },
                ),
                (
                    Hardfork::MuirGlacier,
                    ForkId { hash: ForkHash([0xe0, 0x29, 0xe9, 0x91]), next: 12244000 },
                ),
                (
                    Hardfork::Berlin,
                    ForkId { hash: ForkHash([0x0e, 0xb4, 0x40, 0xf6]), next: 12965000 },
                ),
                (
                    Hardfork::London,
                    ForkId { hash: ForkHash([0xb7, 0x15, 0x07, 0x7d]), next: 13773000 },
                ),
                (
                    Hardfork::ArrowGlacier,
                    ForkId { hash: ForkHash([0x20, 0xc3, 0x27, 0xfc]), next: 15050000 },
                ),
                (
                    Hardfork::GrayGlacier,
                    ForkId { hash: ForkHash([0xf0, 0xaf, 0xd0, 0xe3]), next: 1681338455 },
                ),
                (Hardfork::Shanghai, ForkId { hash: ForkHash([0xdc, 0xe9, 0x6c, 0x2d]), next: 0 }),
            ],
        );
    }

    #[test]
    fn goerli_hardfork_fork_ids() {
        test_hardfork_fork_ids(
            &GOERLI,
            &[
                (
                    Hardfork::Frontier,
                    ForkId { hash: ForkHash([0xa3, 0xf5, 0xab, 0x08]), next: 1561651 },
                ),
                (
                    Hardfork::Homestead,
                    ForkId { hash: ForkHash([0xa3, 0xf5, 0xab, 0x08]), next: 1561651 },
                ),
                (
                    Hardfork::Tangerine,
                    ForkId { hash: ForkHash([0xa3, 0xf5, 0xab, 0x08]), next: 1561651 },
                ),
                (
                    Hardfork::SpuriousDragon,
                    ForkId { hash: ForkHash([0xa3, 0xf5, 0xab, 0x08]), next: 1561651 },
                ),
                (
                    Hardfork::Byzantium,
                    ForkId { hash: ForkHash([0xa3, 0xf5, 0xab, 0x08]), next: 1561651 },
                ),
                (
                    Hardfork::Constantinople,
                    ForkId { hash: ForkHash([0xa3, 0xf5, 0xab, 0x08]), next: 1561651 },
                ),
                (
                    Hardfork::Petersburg,
                    ForkId { hash: ForkHash([0xa3, 0xf5, 0xab, 0x08]), next: 1561651 },
                ),
                (
                    Hardfork::Istanbul,
                    ForkId { hash: ForkHash([0xc2, 0x5e, 0xfa, 0x5c]), next: 4460644 },
                ),
                (
                    Hardfork::Berlin,
                    ForkId { hash: ForkHash([0x75, 0x7a, 0x1c, 0x47]), next: 5062605 },
                ),
                (
                    Hardfork::London,
                    ForkId { hash: ForkHash([0xb8, 0xc6, 0x29, 0x9d]), next: 1678832736 },
                ),
                (Hardfork::Shanghai, ForkId { hash: ForkHash([0xf9, 0x84, 0x3a, 0xbf]), next: 0 }),
            ],
        );
    }

    #[test]
    fn sepolia_hardfork_fork_ids() {
        test_hardfork_fork_ids(
            &SEPOLIA,
            &[
                (
                    Hardfork::Frontier,
                    ForkId { hash: ForkHash([0xfe, 0x33, 0x66, 0xe7]), next: 1735371 },
                ),
                (
                    Hardfork::Homestead,
                    ForkId { hash: ForkHash([0xfe, 0x33, 0x66, 0xe7]), next: 1735371 },
                ),
                (
                    Hardfork::Tangerine,
                    ForkId { hash: ForkHash([0xfe, 0x33, 0x66, 0xe7]), next: 1735371 },
                ),
                (
                    Hardfork::SpuriousDragon,
                    ForkId { hash: ForkHash([0xfe, 0x33, 0x66, 0xe7]), next: 1735371 },
                ),
                (
                    Hardfork::Byzantium,
                    ForkId { hash: ForkHash([0xfe, 0x33, 0x66, 0xe7]), next: 1735371 },
                ),
                (
                    Hardfork::Constantinople,
                    ForkId { hash: ForkHash([0xfe, 0x33, 0x66, 0xe7]), next: 1735371 },
                ),
                (
                    Hardfork::Petersburg,
                    ForkId { hash: ForkHash([0xfe, 0x33, 0x66, 0xe7]), next: 1735371 },
                ),
                (
                    Hardfork::Istanbul,
                    ForkId { hash: ForkHash([0xfe, 0x33, 0x66, 0xe7]), next: 1735371 },
                ),
                (
                    Hardfork::Berlin,
                    ForkId { hash: ForkHash([0xfe, 0x33, 0x66, 0xe7]), next: 1735371 },
                ),
                (
                    Hardfork::London,
                    ForkId { hash: ForkHash([0xfe, 0x33, 0x66, 0xe7]), next: 1735371 },
                ),
                (
                    Hardfork::Paris,
                    ForkId { hash: ForkHash([0xb9, 0x6c, 0xbd, 0x13]), next: 1677557088 },
                ),
                (Hardfork::Shanghai, ForkId { hash: ForkHash([0xf7, 0xf9, 0xbc, 0x08]), next: 0 }),
            ],
        );
    }

    #[test]
    fn mainnet_forkids() {
        test_fork_ids(
            &MAINNET,
            &[
                (
                    Head { number: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0xfc, 0x64, 0xec, 0x04]), next: 1150000 },
                ),
                (
                    Head { number: 1150000, ..Default::default() },
                    ForkId { hash: ForkHash([0x97, 0xc2, 0xc3, 0x4c]), next: 1920000 },
                ),
                (
                    Head { number: 1920000, ..Default::default() },
                    ForkId { hash: ForkHash([0x91, 0xd1, 0xf9, 0x48]), next: 2463000 },
                ),
                (
                    Head { number: 2463000, ..Default::default() },
                    ForkId { hash: ForkHash([0x7a, 0x64, 0xda, 0x13]), next: 2675000 },
                ),
                (
                    Head { number: 2675000, ..Default::default() },
                    ForkId { hash: ForkHash([0x3e, 0xdd, 0x5b, 0x10]), next: 4370000 },
                ),
                (
                    Head { number: 4370000, ..Default::default() },
                    ForkId { hash: ForkHash([0xa0, 0x0b, 0xc3, 0x24]), next: 7280000 },
                ),
                (
                    Head { number: 7280000, ..Default::default() },
                    ForkId { hash: ForkHash([0x66, 0x8d, 0xb0, 0xaf]), next: 9069000 },
                ),
                (
                    Head { number: 9069000, ..Default::default() },
                    ForkId { hash: ForkHash([0x87, 0x9d, 0x6e, 0x30]), next: 9200000 },
                ),
                (
                    Head { number: 9200000, ..Default::default() },
                    ForkId { hash: ForkHash([0xe0, 0x29, 0xe9, 0x91]), next: 12244000 },
                ),
                (
                    Head { number: 12244000, ..Default::default() },
                    ForkId { hash: ForkHash([0x0e, 0xb4, 0x40, 0xf6]), next: 12965000 },
                ),
                (
                    Head { number: 12965000, ..Default::default() },
                    ForkId { hash: ForkHash([0xb7, 0x15, 0x07, 0x7d]), next: 13773000 },
                ),
                (
                    Head { number: 13773000, ..Default::default() },
                    ForkId { hash: ForkHash([0x20, 0xc3, 0x27, 0xfc]), next: 15050000 },
                ),
                (
                    Head { number: 15050000, ..Default::default() },
                    ForkId { hash: ForkHash([0xf0, 0xaf, 0xd0, 0xe3]), next: 1681338455 },
                ),
                // First Shanghai block
                (
                    Head { number: 20000000, timestamp: 1681338455, ..Default::default() },
                    ForkId { hash: ForkHash([0xdc, 0xe9, 0x6c, 0x2d]), next: 0 },
                ),
                // Future Shanghai block
                (
                    Head { number: 20000000, timestamp: 2000000000, ..Default::default() },
                    ForkId { hash: ForkHash([0xdc, 0xe9, 0x6c, 0x2d]), next: 0 },
                ),
            ],
        );
    }

    #[test]
    fn goerli_forkids() {
        test_fork_ids(
            &GOERLI,
            &[
                (
                    Head { number: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0xa3, 0xf5, 0xab, 0x08]), next: 1561651 },
                ),
                (
                    Head { number: 1561650, ..Default::default() },
                    ForkId { hash: ForkHash([0xa3, 0xf5, 0xab, 0x08]), next: 1561651 },
                ),
                (
                    Head { number: 1561651, ..Default::default() },
                    ForkId { hash: ForkHash([0xc2, 0x5e, 0xfa, 0x5c]), next: 4460644 },
                ),
                (
                    Head { number: 4460643, ..Default::default() },
                    ForkId { hash: ForkHash([0xc2, 0x5e, 0xfa, 0x5c]), next: 4460644 },
                ),
                (
                    Head { number: 4460644, ..Default::default() },
                    ForkId { hash: ForkHash([0x75, 0x7a, 0x1c, 0x47]), next: 5062605 },
                ),
                (
                    Head { number: 5062605, ..Default::default() },
                    ForkId { hash: ForkHash([0xb8, 0xc6, 0x29, 0x9d]), next: 1678832736 },
                ),
                (
                    Head { number: 6000000, timestamp: 1678832735, ..Default::default() },
                    ForkId { hash: ForkHash([0xb8, 0xc6, 0x29, 0x9d]), next: 1678832736 },
                ),
                // First Shanghai block
                (
                    Head { number: 6000001, timestamp: 1678832736, ..Default::default() },
                    ForkId { hash: ForkHash([0xf9, 0x84, 0x3a, 0xbf]), next: 0 },
                ),
                // Future Shanghai block
                (
                    Head { number: 6500000, timestamp: 1678832736, ..Default::default() },
                    ForkId { hash: ForkHash([0xf9, 0x84, 0x3a, 0xbf]), next: 0 },
                ),
            ],
        );
    }

    #[test]
    fn sepolia_forkids() {
        test_fork_ids(
            &SEPOLIA,
            &[
                (
                    Head { number: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0xfe, 0x33, 0x66, 0xe7]), next: 1735371 },
                ),
                (
                    Head { number: 1735370, ..Default::default() },
                    ForkId { hash: ForkHash([0xfe, 0x33, 0x66, 0xe7]), next: 1735371 },
                ),
                (
                    Head { number: 1735371, ..Default::default() },
                    ForkId { hash: ForkHash([0xb9, 0x6c, 0xbd, 0x13]), next: 1677557088 },
                ),
                (
                    Head { number: 1735372, timestamp: 1677557087, ..Default::default() },
                    ForkId { hash: ForkHash([0xb9, 0x6c, 0xbd, 0x13]), next: 1677557088 },
                ),
                (
                    Head { number: 1735372, timestamp: 1677557088, ..Default::default() },
                    ForkId { hash: ForkHash([0xf7, 0xf9, 0xbc, 0x08]), next: 0 },
                ),
            ],
        );
    }

    #[test]
    fn dev_forkids() {
        test_fork_ids(
            &DEV,
            &[(
                Head { number: 0, ..Default::default() },
                ForkId { hash: ForkHash([0x45, 0xb8, 0x36, 0x12]), next: 0 },
            )],
        )
    }

    #[cfg(feature = "optimism")]
    #[test]
    fn optimism_goerli_forkids() {
        test_fork_ids(
            &OP_GOERLI,
            &[
                (
                    Head { number: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0x6d, 0x63, 0x76, 0xbe]), next: 4061224 },
                ),
                (
                    Head { number: 4061223, ..Default::default() },
                    ForkId { hash: ForkHash([0x6d, 0x63, 0x76, 0xbe]), next: 4061224 },
                ),
                (
                    Head { number: 4061224, timestamp: 1679079599, ..Default::default() },
                    ForkId { hash: ForkHash([0x03, 0x47, 0x85, 0x69]), next: 1679079600 },
                ),
                (
                    Head { number: 4061225, timestamp: 1679079600, ..Default::default() },
                    ForkId { hash: ForkHash([0x6d, 0x43, 0x1d, 0x6c]), next: 1699981200 },
                ),
                (
                    Head { number: 4061226, timestamp: 1699981199, ..Default::default() },
                    ForkId { hash: ForkHash([0x6d, 0x43, 0x1d, 0x6c]), next: 1699981200 },
                ),
                (
                    Head { number: 4061227, timestamp: 1699981200, ..Default::default() },
                    ForkId { hash: ForkHash([0x7f, 0x4a, 0x72, 0x1f]), next: 0 },
                ),
            ],
        );
    }

    #[cfg(feature = "optimism")]
    #[test]
    fn base_goerli_forkids() {
        test_fork_ids(
            &BASE_GOERLI,
            &[
                (
                    Head { number: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0xd4, 0x0c, 0x23, 0x50]), next: 1683219600 },
                ),
                (
                    Head { number: 1, timestamp: 1683219599, ..Default::default() },
                    ForkId { hash: ForkHash([0xd4, 0x0c, 0x23, 0x50]), next: 1683219600 },
                ),
                (
                    Head { number: 2, timestamp: 1683219600, ..Default::default() },
                    ForkId { hash: ForkHash([0xd5, 0x45, 0x43, 0x5d]), next: 1699981200 },
                ),
                (
                    Head { number: 3, timestamp: 1699981199, ..Default::default() },
                    ForkId { hash: ForkHash([0xd5, 0x45, 0x43, 0x5d]), next: 1699981200 },
                ),
                (
                    Head { number: 4, timestamp: 1699981200, ..Default::default() },
                    ForkId { hash: ForkHash([0xb3, 0x29, 0x13, 0xde]), next: 0 },
                ),
            ],
        );
    }

    /// Checks that time-based forks work
    ///
    /// This is based off of the test vectors here: https://github.com/ethereum/go-ethereum/blob/5c8cc10d1e05c23ff1108022f4150749e73c0ca1/core/forkid/forkid_test.go#L155-L188
    #[test]
    fn timestamped_forks() {
        let mainnet_with_shanghai = ChainSpecBuilder::mainnet()
            .with_fork(Hardfork::Shanghai, ForkCondition::Timestamp(1668000000))
            .build();
        test_fork_ids(
            &mainnet_with_shanghai,
            &[
                (
                    Head { number: 0, timestamp: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0xfc, 0x64, 0xec, 0x04]), next: 1150000 },
                ), // Unsynced
                (
                    Head { number: 1149999, timestamp: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0xfc, 0x64, 0xec, 0x04]), next: 1150000 },
                ), // Last Frontier block
                (
                    Head { number: 1150000, timestamp: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0x97, 0xc2, 0xc3, 0x4c]), next: 1920000 },
                ), // First Homestead block
                (
                    Head { number: 1919999, timestamp: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0x97, 0xc2, 0xc3, 0x4c]), next: 1920000 },
                ), // Last Homestead block
                (
                    Head { number: 1920000, timestamp: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0x91, 0xd1, 0xf9, 0x48]), next: 2463000 },
                ), // First DAO block
                (
                    Head { number: 2462999, timestamp: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0x91, 0xd1, 0xf9, 0x48]), next: 2463000 },
                ), // Last DAO block
                (
                    Head { number: 2463000, timestamp: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0x7a, 0x64, 0xda, 0x13]), next: 2675000 },
                ), // First Tangerine block
                (
                    Head { number: 2674999, timestamp: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0x7a, 0x64, 0xda, 0x13]), next: 2675000 },
                ), // Last Tangerine block
                (
                    Head { number: 2675000, timestamp: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0x3e, 0xdd, 0x5b, 0x10]), next: 4370000 },
                ), // First Spurious block
                (
                    Head { number: 4369999, timestamp: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0x3e, 0xdd, 0x5b, 0x10]), next: 4370000 },
                ), // Last Spurious block
                (
                    Head { number: 4370000, timestamp: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0xa0, 0x0b, 0xc3, 0x24]), next: 7280000 },
                ), // First Byzantium block
                (
                    Head { number: 7279999, timestamp: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0xa0, 0x0b, 0xc3, 0x24]), next: 7280000 },
                ), // Last Byzantium block
                (
                    Head { number: 7280000, timestamp: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0x66, 0x8d, 0xb0, 0xaf]), next: 9069000 },
                ), // First and last Constantinople, first Petersburg block
                (
                    Head { number: 9068999, timestamp: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0x66, 0x8d, 0xb0, 0xaf]), next: 9069000 },
                ), // Last Petersburg block
                (
                    Head { number: 9069000, timestamp: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0x87, 0x9d, 0x6e, 0x30]), next: 9200000 },
                ), // First Istanbul and first Muir Glacier block
                (
                    Head { number: 9199999, timestamp: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0x87, 0x9d, 0x6e, 0x30]), next: 9200000 },
                ), // Last Istanbul and first Muir Glacier block
                (
                    Head { number: 9200000, timestamp: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0xe0, 0x29, 0xe9, 0x91]), next: 12244000 },
                ), // First Muir Glacier block
                (
                    Head { number: 12243999, timestamp: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0xe0, 0x29, 0xe9, 0x91]), next: 12244000 },
                ), // Last Muir Glacier block
                (
                    Head { number: 12244000, timestamp: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0x0e, 0xb4, 0x40, 0xf6]), next: 12965000 },
                ), // First Berlin block
                (
                    Head { number: 12964999, timestamp: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0x0e, 0xb4, 0x40, 0xf6]), next: 12965000 },
                ), // Last Berlin block
                (
                    Head { number: 12965000, timestamp: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0xb7, 0x15, 0x07, 0x7d]), next: 13773000 },
                ), // First London block
                (
                    Head { number: 13772999, timestamp: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0xb7, 0x15, 0x07, 0x7d]), next: 13773000 },
                ), // Last London block
                (
                    Head { number: 13773000, timestamp: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0x20, 0xc3, 0x27, 0xfc]), next: 15050000 },
                ), // First Arrow Glacier block
                (
                    Head { number: 15049999, timestamp: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0x20, 0xc3, 0x27, 0xfc]), next: 15050000 },
                ), // Last Arrow Glacier block
                (
                    Head { number: 15050000, timestamp: 0, ..Default::default() },
                    ForkId { hash: ForkHash([0xf0, 0xaf, 0xd0, 0xe3]), next: 1668000000 },
                ), // First Gray Glacier block
                (
                    Head { number: 19999999, timestamp: 1667999999, ..Default::default() },
                    ForkId { hash: ForkHash([0xf0, 0xaf, 0xd0, 0xe3]), next: 1668000000 },
                ), // Last Gray Glacier block
                (
                    Head { number: 20000000, timestamp: 1668000000, ..Default::default() },
                    ForkId { hash: ForkHash([0x71, 0x14, 0x76, 0x44]), next: 0 },
                ), // First Shanghai block
                (
                    Head { number: 20000000, timestamp: 2668000000, ..Default::default() },
                    ForkId { hash: ForkHash([0x71, 0x14, 0x76, 0x44]), next: 0 },
                ), // Future Shanghai block
            ],
        );
    }

    /// Constructs a [ChainSpec] with the given [ChainSpecBuilder], shanghai, and cancun fork
    /// timestamps.
    fn construct_chainspec(
        builder: ChainSpecBuilder,
        shanghai_time: u64,
        cancun_time: u64,
    ) -> ChainSpec {
        builder
            .with_fork(Hardfork::Shanghai, ForkCondition::Timestamp(shanghai_time))
            .with_fork(Hardfork::Cancun, ForkCondition::Timestamp(cancun_time))
            .build()
    }

    /// Tests that time-based forks which are active at genesis are not included in forkid hash.
    ///
    /// This is based off of the test vectors here:
    /// <https://github.com/ethereum/go-ethereum/blob/2e02c1ffd9dffd1ec9e43c6b66f6c9bd1e556a0b/core/forkid/forkid_test.go#L390-L440>
    #[test]
    fn test_timestamp_fork_in_genesis() {
        let timestamp = 1690475657u64;
        let default_spec_builder = ChainSpecBuilder::default()
            .chain(Chain::Id(1337))
            .genesis(Genesis::default().with_timestamp(timestamp))
            .paris_activated();

        // test format: (chain spec, expected next value) - the forkhash will be determined by the
        // genesis hash of the constructed chainspec
        let tests = [
            (
                construct_chainspec(default_spec_builder.clone(), timestamp - 1, timestamp + 1),
                timestamp + 1,
            ),
            (
                construct_chainspec(default_spec_builder.clone(), timestamp, timestamp + 1),
                timestamp + 1,
            ),
            (
                construct_chainspec(default_spec_builder, timestamp + 1, timestamp + 2),
                timestamp + 1,
            ),
        ];

        for (spec, expected_timestamp) in tests {
            let got_forkid = spec.fork_id(&Head { number: 0, timestamp: 0, ..Default::default() });
            // This is slightly different from the geth test because we use the shanghai timestamp
            // to determine whether or not to include a withdrawals root in the genesis header.
            // This makes the genesis hash different, and as a result makes the ChainSpec fork hash
            // different.
            let genesis_hash = spec.genesis_hash();
            let expected_forkid =
                ForkId { hash: ForkHash::from(genesis_hash), next: expected_timestamp };
            assert_eq!(got_forkid, expected_forkid);
        }
    }

    /// Checks that the fork is not active at a terminal ttd block.
    #[test]
    fn check_terminal_ttd() {
        let chainspec = ChainSpecBuilder::mainnet().build();

        // Check that Paris is not active on terminal PoW block #15537393.
        let terminal_block_ttd = U256::from(58750003716598352816469_u128);
        let terminal_block_difficulty = U256::from(11055787484078698_u128);
        assert!(!chainspec
            .fork(Hardfork::Paris)
            .active_at_ttd(terminal_block_ttd, terminal_block_difficulty));

        // Check that Paris is active on first PoS block #15537394.
        let first_pos_block_ttd = U256::from(58750003716598352816469_u128);
        let first_pos_difficulty = U256::ZERO;
        assert!(chainspec
            .fork(Hardfork::Paris)
            .active_at_ttd(first_pos_block_ttd, first_pos_difficulty));
    }

    #[test]
    fn geth_genesis_with_shanghai() {
        let geth_genesis = r#"
        {
          "config": {
            "chainId": 1337,
            "homesteadBlock": 0,
            "eip150Block": 0,
            "eip150Hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
            "eip155Block": 0,
            "eip158Block": 0,
            "byzantiumBlock": 0,
            "constantinopleBlock": 0,
            "petersburgBlock": 0,
            "istanbulBlock": 0,
            "muirGlacierBlock": 0,
            "berlinBlock": 0,
            "londonBlock": 0,
            "arrowGlacierBlock": 0,
            "grayGlacierBlock": 0,
            "shanghaiTime": 0,
            "terminalTotalDifficulty": 0,
            "terminalTotalDifficultyPassed": true,
            "ethash": {}
          },
          "nonce": "0x0",
          "timestamp": "0x0",
          "extraData": "0x",
          "gasLimit": "0x4c4b40",
          "difficulty": "0x1",
          "mixHash": "0x0000000000000000000000000000000000000000000000000000000000000000",
          "coinbase": "0x0000000000000000000000000000000000000000",
          "alloc": {
            "658bdf435d810c91414ec09147daa6db62406379": {
              "balance": "0x487a9a304539440000"
            },
            "aa00000000000000000000000000000000000000": {
              "code": "0x6042",
              "storage": {
                "0x0000000000000000000000000000000000000000000000000000000000000000": "0x0000000000000000000000000000000000000000000000000000000000000000",
                "0x0100000000000000000000000000000000000000000000000000000000000000": "0x0100000000000000000000000000000000000000000000000000000000000000",
                "0x0200000000000000000000000000000000000000000000000000000000000000": "0x0200000000000000000000000000000000000000000000000000000000000000",
                "0x0300000000000000000000000000000000000000000000000000000000000000": "0x0000000000000000000000000000000000000000000000000000000000000303"
              },
              "balance": "0x1",
              "nonce": "0x1"
            },
            "bb00000000000000000000000000000000000000": {
              "code": "0x600154600354",
              "storage": {
                "0x0000000000000000000000000000000000000000000000000000000000000000": "0x0000000000000000000000000000000000000000000000000000000000000000",
                "0x0100000000000000000000000000000000000000000000000000000000000000": "0x0100000000000000000000000000000000000000000000000000000000000000",
                "0x0200000000000000000000000000000000000000000000000000000000000000": "0x0200000000000000000000000000000000000000000000000000000000000000",
                "0x0300000000000000000000000000000000000000000000000000000000000000": "0x0000000000000000000000000000000000000000000000000000000000000303"
              },
              "balance": "0x2",
              "nonce": "0x1"
            }
          },
          "number": "0x0",
          "gasUsed": "0x0",
          "parentHash": "0x0000000000000000000000000000000000000000000000000000000000000000",
          "baseFeePerGas": "0x3b9aca00"
        }
        "#;

        let genesis: Genesis = serde_json::from_str(geth_genesis).unwrap();
        let chainspec = ChainSpec::from(genesis);

        // assert a bunch of hardforks that should be set
        assert_eq!(
            chainspec.hardforks.get(&Hardfork::Homestead).unwrap(),
            &ForkCondition::Block(0)
        );
        assert_eq!(
            chainspec.hardforks.get(&Hardfork::Tangerine).unwrap(),
            &ForkCondition::Block(0)
        );
        assert_eq!(
            chainspec.hardforks.get(&Hardfork::SpuriousDragon).unwrap(),
            &ForkCondition::Block(0)
        );
        assert_eq!(
            chainspec.hardforks.get(&Hardfork::Byzantium).unwrap(),
            &ForkCondition::Block(0)
        );
        assert_eq!(
            chainspec.hardforks.get(&Hardfork::Constantinople).unwrap(),
            &ForkCondition::Block(0)
        );
        assert_eq!(
            chainspec.hardforks.get(&Hardfork::Petersburg).unwrap(),
            &ForkCondition::Block(0)
        );
        assert_eq!(chainspec.hardforks.get(&Hardfork::Istanbul).unwrap(), &ForkCondition::Block(0));
        assert_eq!(
            chainspec.hardforks.get(&Hardfork::MuirGlacier).unwrap(),
            &ForkCondition::Block(0)
        );
        assert_eq!(chainspec.hardforks.get(&Hardfork::Berlin).unwrap(), &ForkCondition::Block(0));
        assert_eq!(chainspec.hardforks.get(&Hardfork::London).unwrap(), &ForkCondition::Block(0));
        assert_eq!(
            chainspec.hardforks.get(&Hardfork::ArrowGlacier).unwrap(),
            &ForkCondition::Block(0)
        );
        assert_eq!(
            chainspec.hardforks.get(&Hardfork::GrayGlacier).unwrap(),
            &ForkCondition::Block(0)
        );

        // including time based hardforks
        assert_eq!(
            chainspec.hardforks.get(&Hardfork::Shanghai).unwrap(),
            &ForkCondition::Timestamp(0)
        );

        // alloc key -> expected rlp mapping
        let key_rlp = vec![
            (hex!("658bdf435d810c91414ec09147daa6db62406379"), &hex!("f84d8089487a9a304539440000a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a0c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470")[..]),
            (hex!("aa00000000000000000000000000000000000000"), &hex!("f8440101a08afc95b7d18a226944b9c2070b6bda1c3a36afcc3730429d47579c94b9fe5850a0ce92c756baff35fa740c3557c1a971fd24d2d35b7c8e067880d50cd86bb0bc99")[..]),
            (hex!("bb00000000000000000000000000000000000000"), &hex!("f8440102a08afc95b7d18a226944b9c2070b6bda1c3a36afcc3730429d47579c94b9fe5850a0e25a53cbb501cec2976b393719c63d832423dd70a458731a0b64e4847bbca7d2")[..]),
        ];

        for (key, expected_rlp) in key_rlp {
            let account = chainspec.genesis.alloc.get(&key).expect("account should exist");
            let mut account_rlp = BytesMut::new();
            TrieAccount::from(account.clone()).encode(&mut account_rlp);
            assert_eq!(account_rlp, expected_rlp)
        }

        assert_eq!(chainspec.genesis_hash, None);
        let expected_state_root: B256 =
            hex!("078dc6061b1d8eaa8493384b59c9c65ceb917201221d08b80c4de6770b6ec7e7").into();
        assert_eq!(chainspec.genesis_header().state_root, expected_state_root);

        let expected_withdrawals_hash: B256 =
            hex!("56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421").into();
        assert_eq!(chainspec.genesis_header().withdrawals_root, Some(expected_withdrawals_hash));

        let expected_hash: B256 =
            hex!("1fc027d65f820d3eef441ebeec139ebe09e471cf98516dce7b5643ccb27f418c").into();
        let hash = chainspec.genesis_hash();
        assert_eq!(hash, expected_hash);
    }

    #[test]
    fn hive_geth_json() {
        let hive_json = r#"
        {
            "nonce": "0x0000000000000042",
            "difficulty": "0x2123456",
            "mixHash": "0x123456789abcdef123456789abcdef123456789abcdef123456789abcdef1234",
            "coinbase": "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "timestamp": "0x123456",
            "parentHash": "0x0000000000000000000000000000000000000000000000000000000000000000",
            "extraData": "0xfafbfcfd",
            "gasLimit": "0x2fefd8",
            "alloc": {
                "dbdbdb2cbd23b783741e8d7fcf51e459b497e4a6": {
                    "balance": "0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
                },
                "e6716f9544a56c530d868e4bfbacb172315bdead": {
                    "balance": "0x11",
                    "code": "0x12"
                },
                "b9c015918bdaba24b4ff057a92a3873d6eb201be": {
                    "balance": "0x21",
                    "storage": {
                        "0x0000000000000000000000000000000000000000000000000000000000000001": "0x22"
                    }
                },
                "1a26338f0d905e295fccb71fa9ea849ffa12aaf4": {
                    "balance": "0x31",
                    "nonce": "0x32"
                },
                "0000000000000000000000000000000000000001": {
                    "balance": "0x41"
                },
                "0000000000000000000000000000000000000002": {
                    "balance": "0x51"
                },
                "0000000000000000000000000000000000000003": {
                    "balance": "0x61"
                },
                "0000000000000000000000000000000000000004": {
                    "balance": "0x71"
                }
            },
            "config": {
                "ethash": {},
                "chainId": 10,
                "homesteadBlock": 0,
                "eip150Block": 0,
                "eip155Block": 0,
                "eip158Block": 0,
                "byzantiumBlock": 0,
                "constantinopleBlock": 0,
                "petersburgBlock": 0,
                "istanbulBlock": 0
            }
        }
        "#;

        let _genesis = serde_json::from_str::<Genesis>(hive_json).unwrap();
        let genesis = serde_json::from_str::<AllGenesisFormats>(hive_json).unwrap();
        let chainspec: ChainSpec = genesis.into();
        assert_eq!(chainspec.genesis_hash, None);
        assert_eq!(chainspec.chain, Chain::Named(NamedChain::Optimism));
        let expected_state_root: B256 =
            hex!("9a6049ac535e3dc7436c189eaa81c73f35abd7f282ab67c32944ff0301d63360").into();
        assert_eq!(chainspec.genesis_header().state_root, expected_state_root);
        let hard_forks = vec![
            Hardfork::Byzantium,
            Hardfork::Homestead,
            Hardfork::Istanbul,
            Hardfork::Petersburg,
            Hardfork::Constantinople,
        ];
        for ref fork in hard_forks {
            assert_eq!(chainspec.hardforks.get(fork).unwrap(), &ForkCondition::Block(0));
        }

        let expected_hash: B256 =
            hex!("5ae31c6522bd5856129f66be3d582b842e4e9faaa87f21cce547128339a9db3c").into();
        let hash = chainspec.genesis_header().hash_slow();
        assert_eq!(hash, expected_hash);
    }

    #[test]
    fn test_parse_genesis_json() {
        let s = r#"{"config":{"ethash":{},"chainId":1337,"homesteadBlock":0,"eip150Block":0,"eip155Block":0,"eip158Block":0,"byzantiumBlock":0,"constantinopleBlock":0,"petersburgBlock":0,"istanbulBlock":0,"berlinBlock":0,"londonBlock":0,"terminalTotalDifficulty":0,"terminalTotalDifficultyPassed":true,"shanghaiTime":0},"nonce":"0x0","timestamp":"0x0","extraData":"0x","gasLimit":"0x4c4b40","difficulty":"0x1","mixHash":"0x0000000000000000000000000000000000000000000000000000000000000000","coinbase":"0x0000000000000000000000000000000000000000","alloc":{"658bdf435d810c91414ec09147daa6db62406379":{"balance":"0x487a9a304539440000"},"aa00000000000000000000000000000000000000":{"code":"0x6042","storage":{"0x0000000000000000000000000000000000000000000000000000000000000000":"0x0000000000000000000000000000000000000000000000000000000000000000","0x0100000000000000000000000000000000000000000000000000000000000000":"0x0100000000000000000000000000000000000000000000000000000000000000","0x0200000000000000000000000000000000000000000000000000000000000000":"0x0200000000000000000000000000000000000000000000000000000000000000","0x0300000000000000000000000000000000000000000000000000000000000000":"0x0000000000000000000000000000000000000000000000000000000000000303"},"balance":"0x1","nonce":"0x1"},"bb00000000000000000000000000000000000000":{"code":"0x600154600354","storage":{"0x0000000000000000000000000000000000000000000000000000000000000000":"0x0000000000000000000000000000000000000000000000000000000000000000","0x0100000000000000000000000000000000000000000000000000000000000000":"0x0100000000000000000000000000000000000000000000000000000000000000","0x0200000000000000000000000000000000000000000000000000000000000000":"0x0200000000000000000000000000000000000000000000000000000000000000","0x0300000000000000000000000000000000000000000000000000000000000000":"0x0000000000000000000000000000000000000000000000000000000000000303"},"balance":"0x2","nonce":"0x1"}},"number":"0x0","gasUsed":"0x0","parentHash":"0x0000000000000000000000000000000000000000000000000000000000000000","baseFeePerGas":"0x1337"}"#;
        let genesis: Genesis = serde_json::from_str(s).unwrap();
        let acc = genesis
            .alloc
            .get(&"0xaa00000000000000000000000000000000000000".parse::<Address>().unwrap())
            .unwrap();
        assert_eq!(acc.balance, U256::from(1));
        assert_eq!(genesis.base_fee_per_gas, Some(0x1337));
    }

    #[test]
    fn test_parse_cancun_genesis_json() {
        let s = r#"{"config":{"ethash":{},"chainId":1337,"homesteadBlock":0,"eip150Block":0,"eip155Block":0,"eip158Block":0,"byzantiumBlock":0,"constantinopleBlock":0,"petersburgBlock":0,"istanbulBlock":0,"berlinBlock":0,"londonBlock":0,"terminalTotalDifficulty":0,"terminalTotalDifficultyPassed":true,"shanghaiTime":0,"cancunTime":4661},"nonce":"0x0","timestamp":"0x0","extraData":"0x","gasLimit":"0x4c4b40","difficulty":"0x1","mixHash":"0x0000000000000000000000000000000000000000000000000000000000000000","coinbase":"0x0000000000000000000000000000000000000000","alloc":{"658bdf435d810c91414ec09147daa6db62406379":{"balance":"0x487a9a304539440000"},"aa00000000000000000000000000000000000000":{"code":"0x6042","storage":{"0x0000000000000000000000000000000000000000000000000000000000000000":"0x0000000000000000000000000000000000000000000000000000000000000000","0x0100000000000000000000000000000000000000000000000000000000000000":"0x0100000000000000000000000000000000000000000000000000000000000000","0x0200000000000000000000000000000000000000000000000000000000000000":"0x0200000000000000000000000000000000000000000000000000000000000000","0x0300000000000000000000000000000000000000000000000000000000000000":"0x0000000000000000000000000000000000000000000000000000000000000303"},"balance":"0x1","nonce":"0x1"},"bb00000000000000000000000000000000000000":{"code":"0x600154600354","storage":{"0x0000000000000000000000000000000000000000000000000000000000000000":"0x0000000000000000000000000000000000000000000000000000000000000000","0x0100000000000000000000000000000000000000000000000000000000000000":"0x0100000000000000000000000000000000000000000000000000000000000000","0x0200000000000000000000000000000000000000000000000000000000000000":"0x0200000000000000000000000000000000000000000000000000000000000000","0x0300000000000000000000000000000000000000000000000000000000000000":"0x0000000000000000000000000000000000000000000000000000000000000303"},"balance":"0x2","nonce":"0x1"}},"number":"0x0","gasUsed":"0x0","parentHash":"0x0000000000000000000000000000000000000000000000000000000000000000","baseFeePerGas":"0x3b9aca00"}"#;
        let genesis: Genesis = serde_json::from_str(s).unwrap();
        let acc = genesis
            .alloc
            .get(&"0xaa00000000000000000000000000000000000000".parse::<Address>().unwrap())
            .unwrap();
        assert_eq!(acc.balance, U256::from(1));
        // assert that the cancun time was picked up
        assert_eq!(genesis.config.cancun_time, Some(4661));
    }

    #[test]
    fn test_parse_cancun_genesis_all_formats() {
        let s = r#"{"config":{"ethash":{},"chainId":1337,"homesteadBlock":0,"eip150Block":0,"eip155Block":0,"eip158Block":0,"byzantiumBlock":0,"constantinopleBlock":0,"petersburgBlock":0,"istanbulBlock":0,"berlinBlock":0,"londonBlock":0,"terminalTotalDifficulty":0,"terminalTotalDifficultyPassed":true,"shanghaiTime":0,"cancunTime":4661},"nonce":"0x0","timestamp":"0x0","extraData":"0x","gasLimit":"0x4c4b40","difficulty":"0x1","mixHash":"0x0000000000000000000000000000000000000000000000000000000000000000","coinbase":"0x0000000000000000000000000000000000000000","alloc":{"658bdf435d810c91414ec09147daa6db62406379":{"balance":"0x487a9a304539440000"},"aa00000000000000000000000000000000000000":{"code":"0x6042","storage":{"0x0000000000000000000000000000000000000000000000000000000000000000":"0x0000000000000000000000000000000000000000000000000000000000000000","0x0100000000000000000000000000000000000000000000000000000000000000":"0x0100000000000000000000000000000000000000000000000000000000000000","0x0200000000000000000000000000000000000000000000000000000000000000":"0x0200000000000000000000000000000000000000000000000000000000000000","0x0300000000000000000000000000000000000000000000000000000000000000":"0x0000000000000000000000000000000000000000000000000000000000000303"},"balance":"0x1","nonce":"0x1"},"bb00000000000000000000000000000000000000":{"code":"0x600154600354","storage":{"0x0000000000000000000000000000000000000000000000000000000000000000":"0x0000000000000000000000000000000000000000000000000000000000000000","0x0100000000000000000000000000000000000000000000000000000000000000":"0x0100000000000000000000000000000000000000000000000000000000000000","0x0200000000000000000000000000000000000000000000000000000000000000":"0x0200000000000000000000000000000000000000000000000000000000000000","0x0300000000000000000000000000000000000000000000000000000000000000":"0x0000000000000000000000000000000000000000000000000000000000000303"},"balance":"0x2","nonce":"0x1"}},"number":"0x0","gasUsed":"0x0","parentHash":"0x0000000000000000000000000000000000000000000000000000000000000000","baseFeePerGas":"0x3b9aca00"}"#;
        let genesis: AllGenesisFormats = serde_json::from_str(s).unwrap();

        // this should be the genesis format
        let genesis = match genesis {
            AllGenesisFormats::Geth(genesis) => genesis,
            _ => panic!("expected geth genesis format"),
        };

        // assert that the alloc was picked up
        let acc = genesis
            .alloc
            .get(&"0xaa00000000000000000000000000000000000000".parse::<Address>().unwrap())
            .unwrap();
        assert_eq!(acc.balance, U256::from(1));
        // assert that the cancun time was picked up
        assert_eq!(genesis.config.cancun_time, Some(4661));
    }

    #[test]
    fn test_default_cancun_header_forkhash() {
        // set the gas limit from the hive test genesis according to the hash
        let genesis = Genesis { gas_limit: 0x2fefd8u64, ..Default::default() };
        let default_chainspec = ChainSpecBuilder::default()
            .chain(Chain::Id(1337))
            .genesis(genesis)
            .cancun_activated()
            .build();
        let mut header = default_chainspec.genesis_header();

        // set the state root to the same as in the hive test the hash was pulled from
        header.state_root =
            B256::from_str("0x62e2595e017f0ca23e08d17221010721a71c3ae932f4ea3cb12117786bb392d4")
                .unwrap();

        // shanghai is activated so we should have a withdrawals root
        assert_eq!(header.withdrawals_root, Some(EMPTY_WITHDRAWALS));

        // cancun is activated so we should have a zero parent beacon block root, zero blob gas
        // used, and zero excess blob gas
        assert_eq!(header.parent_beacon_block_root, Some(B256::ZERO));
        assert_eq!(header.blob_gas_used, Some(0));
        assert_eq!(header.excess_blob_gas, Some(0));
        println!("header: {:?}", header);

        // check the genesis hash
        let genesis_hash = header.hash_slow();
        let expected_hash =
            b256!("16bb7c59613a5bad3f7c04a852fd056545ade2483968d9a25a1abb05af0c4d37");
        assert_eq!(genesis_hash, expected_hash);

        // check that the forkhash is correct
        let expected_forkhash = ForkHash(hex!("8062457a"));
        assert_eq!(ForkHash::from(genesis_hash), expected_forkhash);
    }

    #[test]
    fn holesky_paris_activated_at_genesis() {
        assert!(HOLESKY
            .fork(Hardfork::Paris)
            .active_at_ttd(HOLESKY.genesis.difficulty, HOLESKY.genesis.difficulty));
    }

    #[test]
    fn test_all_genesis_formats_deserialization() {
        // custom genesis with chain config
        let config = ChainConfig {
            chain_id: 2600,
            homestead_block: Some(0),
            eip150_block: Some(0),
            eip155_block: Some(0),
            eip158_block: Some(0),
            byzantium_block: Some(0),
            constantinople_block: Some(0),
            petersburg_block: Some(0),
            istanbul_block: Some(0),
            berlin_block: Some(0),
            london_block: Some(0),
            shanghai_time: Some(0),
            terminal_total_difficulty: Some(U256::ZERO),
            terminal_total_difficulty_passed: true,
            ..Default::default()
        };
        // genesis
        let genesis = Genesis {
            config,
            nonce: 0,
            timestamp: 1698688670,
            gas_limit: 5000,
            difficulty: U256::ZERO,
            mix_hash: B256::ZERO,
            coinbase: Address::ZERO,
            ..Default::default()
        };

        // seed accounts after genesis struct created
        let address = hex!("6Be02d1d3665660d22FF9624b7BE0551ee1Ac91b").into();
        let account = GenesisAccount::default().with_balance(U256::from(33));
        let genesis = genesis.extend_accounts(HashMap::from([(address, account)]));

        // ensure genesis is deserialized correctly
        let serialized_genesis = serde_json::to_string(&genesis).unwrap();
        let deserialized_genesis: AllGenesisFormats =
            serde_json::from_str(&serialized_genesis).unwrap();
        assert!(matches!(deserialized_genesis, AllGenesisFormats::Geth(_)));

        // build chain
        let chain_spec = ChainSpecBuilder::default()
            .chain(2600.into())
            .genesis(genesis)
            .cancun_activated()
            .build();

        // ensure chain spec is deserialized correctly
        let serialized_chain_spec = serde_json::to_string(&chain_spec).unwrap();
        let deserialized_chain_spec: AllGenesisFormats =
            serde_json::from_str(&serialized_chain_spec).unwrap();
        assert!(matches!(deserialized_chain_spec, AllGenesisFormats::Reth(_)))
    }

    #[test]
    fn check_fork_id_chainspec_with_fork_condition_never() {
        let spec = ChainSpec {
            chain: Chain::mainnet(),
            genesis: Genesis::default(),
            genesis_hash: None,
            hardforks: BTreeMap::from([(Hardfork::Frontier, ForkCondition::Never)]),
            paris_block_and_final_difficulty: None,
            deposit_contract: None,
            ..Default::default()
        };

        assert_eq!(spec.hardfork_fork_id(Hardfork::Frontier), None);
    }

    #[test]
    fn check_fork_filter_chainspec_with_fork_condition_never() {
        let spec = ChainSpec {
            chain: Chain::mainnet(),
            genesis: Genesis::default(),
            genesis_hash: None,
            hardforks: BTreeMap::from([(Hardfork::Shanghai, ForkCondition::Never)]),
            paris_block_and_final_difficulty: None,
            deposit_contract: None,
            ..Default::default()
        };

        assert_eq!(spec.hardfork_fork_filter(Hardfork::Shanghai), None);
    }
}
