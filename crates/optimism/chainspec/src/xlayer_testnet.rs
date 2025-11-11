// ! Chain specification for the X Layer Testnet network.

use alloc::sync::Arc;

use alloy_chains::Chain;
use alloy_primitives::{b256, B256, U256};
use reth_chainspec::{BaseFeeParams, BaseFeeParamsKind, ChainSpec};
use reth_ethereum_forks::{EthereumHardfork, Hardfork};
use reth_optimism_forks::{OpHardfork, XLAYER_TESTNET_HARDFORKS};
use reth_primitives_traits::SealedHeader;

use crate::{LazyLock, OpChainSpec, make_op_genesis_header};

/// X Layer Testnet genesis hash
///
/// Computed from the genesis block header.
/// This value is hardcoded to avoid expensive computation on every startup.
pub(crate) const XLAYER_TESTNET_GENESIS_HASH: B256 = b256!("ccb16eb07b7a718c2ee374df57b0e28c9ac9d8d18ca6d3204cfbba661067855a");

/// X Layer Testnet genesis state root
///
/// The Merkle Patricia Trie root of all 6,234,122 accounts in the genesis alloc.
/// This value is hardcoded to avoid expensive computation on every startup.
pub(crate) const XLAYER_TESTNET_STATE_ROOT: B256 = b256!("3de62c8ade3d3adaa88d48a3ffeebd7c8b6c5b81906d706c22f02f0d2dd3b8fa");

/// X Layer testnet chain id from the published `genesis-testnet.json`.
const XLAYER_TESTNET_CHAIN_ID: u64 = 1952;

/// X Layer testnet EIP-1559 parameters.
///
/// Same as mainnet: see `config.optimism` in `genesis-testnet.json`.
const XLAYER_TESTNET_BASE_FEE_DENOMINATOR: u128 = 100_000_000;
const XLAYER_TESTNET_BASE_FEE_ELASTICITY: u128 = 1;

/// X Layer testnet base fee params (same for London and Canyon forks).
const XLAYER_TESTNET_BASE_FEE_PARAMS: BaseFeeParams = BaseFeeParams::new(
    XLAYER_TESTNET_BASE_FEE_DENOMINATOR,
    XLAYER_TESTNET_BASE_FEE_ELASTICITY,
);

/// The X Layer testnet spec
pub static XLAYER_TESTNET: LazyLock<Arc<OpChainSpec>> = LazyLock::new(|| {
    // Minimal genesis contains empty alloc field for fast loading
    let genesis = serde_json::from_str(include_str!("../res/genesis/xlayer_testnet.json"))
        .expect("Can't deserialize X Layer Testnet genesis json");
    let hardforks = XLAYER_TESTNET_HARDFORKS.clone();

    // Build genesis header using standard helper, then override state_root with pre-computed value
    let mut genesis_header = make_op_genesis_header(&genesis, &hardforks);
    genesis_header.state_root = XLAYER_TESTNET_STATE_ROOT;
    let genesis_header = SealedHeader::new(genesis_header, XLAYER_TESTNET_GENESIS_HASH);

    OpChainSpec {
        inner: ChainSpec {
            chain: Chain::from_id(XLAYER_TESTNET_CHAIN_ID),
            genesis_header,
            genesis,
            paris_block_and_final_difficulty: Some((0, U256::from(0))),
            hardforks,
            base_fee_params: BaseFeeParamsKind::Variable(
                vec![
                    (EthereumHardfork::London.boxed(), XLAYER_TESTNET_BASE_FEE_PARAMS),
                    (OpHardfork::Canyon.boxed(), XLAYER_TESTNET_BASE_FEE_PARAMS),
                ]
                .into(),
            ),
            ..Default::default()
        },
    }
    .into()
});

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_genesis::Genesis;
    use alloy_primitives::hex;
    use reth_ethereum_forks::EthereumHardfork;
    use reth_optimism_forks::OpHardfork;

    fn parse_genesis() -> Genesis {
        serde_json::from_str(include_str!("../res/genesis/xlayer_testnet.json"))
            .expect("Failed to parse xlayer_testnet.json")
    }

    #[test]
    fn test_xlayer_testnet_chain_id() {
        assert_eq!(XLAYER_TESTNET.chain().id(), 1952);
    }

    #[test]
    fn test_xlayer_testnet_genesis_hash() {
        assert_eq!(XLAYER_TESTNET.genesis_hash(), XLAYER_TESTNET_GENESIS_HASH);
    }

    #[test]
    fn test_xlayer_testnet_state_root() {
        assert_eq!(XLAYER_TESTNET.genesis_header().state_root, XLAYER_TESTNET_STATE_ROOT);
    }

    #[test]
    fn test_xlayer_testnet_genesis_block_number() {
        assert_eq!(XLAYER_TESTNET.genesis_header().number, 12241700);
    }

    #[test]
    fn test_xlayer_testnet_hardforks() {
        let spec = &*XLAYER_TESTNET;
        assert!(spec.fork(EthereumHardfork::Shanghai).active_at_timestamp(0));
        assert!(spec.fork(EthereumHardfork::Cancun).active_at_timestamp(0));
        assert!(spec.fork(OpHardfork::Bedrock).active_at_block(0));
        assert!(spec.fork(OpHardfork::Isthmus).active_at_timestamp(0));
    }

    #[test]
    fn test_xlayer_testnet_base_fee_params() {
        assert_eq!(
            XLAYER_TESTNET.base_fee_params_at_timestamp(0),
            BaseFeeParams::new(XLAYER_TESTNET_BASE_FEE_DENOMINATOR, XLAYER_TESTNET_BASE_FEE_ELASTICITY)
        );
    }

    #[test]
    fn test_xlayer_testnet_fast_loading() {
        assert_eq!(XLAYER_TESTNET.genesis().alloc.len(), 0);
    }

    #[test]
    fn test_xlayer_testnet_paris_activated() {
        assert_eq!(XLAYER_TESTNET.get_final_paris_total_difficulty(), Some(U256::ZERO));
    }

    #[test]
    fn test_xlayer_testnet_canyon_base_fee_unchanged() {
        let spec = &*XLAYER_TESTNET;
        let london = spec.base_fee_params_at_timestamp(0);
        let canyon = spec.base_fee_params_at_timestamp(1);
        assert_eq!(london, canyon);
        assert_eq!(canyon, XLAYER_TESTNET_BASE_FEE_PARAMS);
    }

    #[test]
    fn test_xlayer_testnet_genesis_header_fields() {
        let header = XLAYER_TESTNET.genesis_header();
        assert_eq!(header.withdrawals_root, Some(alloy_consensus::constants::EMPTY_WITHDRAWALS));
        assert_eq!(header.parent_beacon_block_root, Some(B256::ZERO));
        assert_eq!(header.requests_hash, Some(alloy_eips::eip7685::EMPTY_REQUESTS_HASH));
    }

    #[test]
    fn test_xlayer_testnet_all_hardforks_active() {
        let spec = &*XLAYER_TESTNET;
        let ts = spec.genesis_header().timestamp;
        // Ethereum hardforks
        assert!(spec.fork(EthereumHardfork::London).active_at_block(0));
        assert!(spec.fork(EthereumHardfork::Shanghai).active_at_timestamp(ts));
        assert!(spec.fork(EthereumHardfork::Cancun).active_at_timestamp(ts));
        assert!(spec.fork(EthereumHardfork::Prague).active_at_timestamp(ts));
        // Optimism hardforks
        assert!(spec.fork(OpHardfork::Bedrock).active_at_block(0));
        assert!(spec.fork(OpHardfork::Regolith).active_at_timestamp(ts));
        assert!(spec.fork(OpHardfork::Canyon).active_at_timestamp(ts));
        assert!(spec.fork(OpHardfork::Ecotone).active_at_timestamp(ts));
        assert!(spec.fork(OpHardfork::Fjord).active_at_timestamp(ts));
        assert!(spec.fork(OpHardfork::Granite).active_at_timestamp(ts));
        assert!(spec.fork(OpHardfork::Holocene).active_at_timestamp(ts));
        assert!(spec.fork(OpHardfork::Isthmus).active_at_timestamp(ts));
    }

    #[test]
    fn test_xlayer_testnet_constants_match_spec() {
        assert_eq!(XLAYER_TESTNET.chain().id(), XLAYER_TESTNET_CHAIN_ID);
        assert_eq!(
            XLAYER_TESTNET.base_fee_params_at_timestamp(0),
            BaseFeeParams::new(XLAYER_TESTNET_BASE_FEE_DENOMINATOR, XLAYER_TESTNET_BASE_FEE_ELASTICITY)
        );
    }

    #[test]
    fn test_xlayer_testnet_json_config_consistency() {
        let genesis = parse_genesis();
        assert_eq!(genesis.config.chain_id, XLAYER_TESTNET_CHAIN_ID);
        assert_eq!(genesis.number, Some(12241700));
        assert_eq!(genesis.timestamp, 0x68f22879);
        assert_eq!(genesis.extra_data.as_ref(), hex!("00000000fa00000006").as_ref());
        assert_eq!(genesis.gas_limit, 0x1c9c380);
        assert_eq!(genesis.difficulty, U256::ZERO);
        assert_eq!(genesis.nonce, 0);
        assert_eq!(genesis.mix_hash, B256::ZERO);
        assert_eq!(genesis.coinbase.to_string(), "0x4200000000000000000000000000000000000011");
        assert_eq!(genesis.parent_hash, Some(b256!("dc9ee94a48a651d8264f9fe973c6a3e4b9c74614d233a9e523d27edbfc645158")));
        assert_eq!(genesis.base_fee_per_gas.map(|fee| fee as u64), Some(0x5f5e100u64));
        assert_eq!(genesis.excess_blob_gas, Some(0));
        assert_eq!(genesis.blob_gas_used, Some(0));
    }

    #[test]
    fn test_xlayer_testnet_json_optimism_config() {
        let genesis = parse_genesis();
        let cfg = genesis.config.extra_fields.get("optimism").expect("optimism config must exist");
        assert_eq!(cfg.get("eip1559Elasticity").and_then(|v| v.as_u64()).unwrap() as u128, XLAYER_TESTNET_BASE_FEE_ELASTICITY);
        assert_eq!(cfg.get("eip1559Denominator").and_then(|v| v.as_u64()).unwrap() as u128, XLAYER_TESTNET_BASE_FEE_DENOMINATOR);
        assert_eq!(cfg.get("eip1559DenominatorCanyon").and_then(|v| v.as_u64()).unwrap() as u128, XLAYER_TESTNET_BASE_FEE_DENOMINATOR);
    }

    #[test]
    fn test_xlayer_testnet_json_hardforks_warning() {
        let genesis = parse_genesis();
        // WARNING: Hardfork times in JSON are overridden by XLAYER_TESTNET_HARDFORKS
        assert_eq!(genesis.config.extra_fields.get("legacyXLayerBlock").and_then(|v| v.as_u64()), Some(12241700));
        assert_eq!(genesis.config.shanghai_time, Some(0));
        assert_eq!(genesis.config.cancun_time, Some(0));
    }

    #[test]
    fn test_xlayer_testnet_genesis_header_matches_json() {
        let header = XLAYER_TESTNET.genesis_header();
        let genesis = parse_genesis();
        // Verify header fields match JSON (except state_root which is hardcoded)
        assert_eq!(header.number, genesis.number.unwrap_or_default());
        assert_eq!(header.timestamp, genesis.timestamp);
        assert_eq!(header.extra_data, genesis.extra_data);
        assert_eq!(header.gas_limit, genesis.gas_limit);
        assert_eq!(header.difficulty, genesis.difficulty);
        assert_eq!(header.nonce, alloy_primitives::B64::from(genesis.nonce));
        assert_eq!(header.mix_hash, genesis.mix_hash);
        assert_eq!(header.beneficiary, genesis.coinbase);
        assert_eq!(header.parent_hash, genesis.parent_hash.unwrap_or_default());
        assert_eq!(header.base_fee_per_gas, genesis.base_fee_per_gas.map(|fee| fee as u64));
        // NOTE: state_root is hardcoded, not read from JSON
        assert_eq!(header.state_root, XLAYER_TESTNET_STATE_ROOT);
    }
}

