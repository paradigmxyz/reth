use crate::{ChainSpec, ChainSpecBuilder, DepositContract, EthereumHardforks};
use alloy_chains::Chain;
use alloy_genesis::Genesis;
use alloy_primitives::{address, b256, Address, B256, U256};
use once_cell::sync::Lazy;

/// The Hyperliquid mainnet chain spec.
pub static HYPERLIQUID: Lazy<Arc<ChainSpec>> = Lazy::new(|| {
    ChainSpecBuilder::default()
        .chain(Chain::from_id(8453)) // Hyperliquid chain ID
        .genesis(HYPERLIQUID_GENESIS.clone())
        .with_deposit_contract(HYPERLIQUID_DEPOSIT_CONTRACT)
        .build()
});

/// The Hyperliquid testnet chain spec.
pub static HYPERLIQUID_TESTNET: Lazy<Arc<ChainSpec>> = Lazy::new(|| {
    ChainSpecBuilder::default()
        .chain(Chain::from_id(84532)) // Hyperliquid testnet chain ID
        .genesis(HYPERLIQUID_TESTNET_GENESIS.clone())
        .with_deposit_contract(HYPERLIQUID_TESTNET_DEPOSIT_CONTRACT)
        .build()
});

/// Hyperliquid deposit contract address
const HYPERLIQUID_DEPOSIT_CONTRACT: Address = address!("2222222222222222222222222222222222222222");

/// Hyperliquid testnet deposit contract address  
const HYPERLIQUID_TESTNET_DEPOSIT_CONTRACT: Address = address!("2222222222222222222222222222222222222222");

static HYPERLIQUID_GENESIS: Lazy<Genesis> = Lazy::new(|| {
    serde_json::from_str(include_str!("../genesis/hyperliquid.json"))
        .expect("Can't deserialize Hyperliquid genesis json")
});

static HYPERLIQUID_TESTNET_GENESIS: Lazy<Genesis> = Lazy::new(|| {
    serde_json::from_str(include_str!("../genesis/hyperliquid_testnet.json"))
        .expect("Can't deserialize Hyperliquid testnet genesis json")
});