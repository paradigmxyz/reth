// ! Chain specification for the X Layer Mainnet network.

use alloc::{sync::Arc, vec};

use alloy_chains::Chain;
use alloy_primitives::{b256, B256, U256};
use reth_chainspec::{BaseFeeParams, BaseFeeParamsKind, ChainSpec};
use reth_ethereum_forks::{EthereumHardfork, Hardfork};
use reth_optimism_forks::{OpHardfork, XLAYER_MAINNET_HARDFORKS};
use reth_primitives_traits::SealedHeader;

use crate::{make_op_genesis_header, LazyLock, OpChainSpec};

/// X Layer Mainnet genesis hash
///
/// Computed from the genesis block header.
/// This value is hardcoded to avoid expensive computation on every startup.
pub(crate) const XLAYER_MAINNET_GENESIS_HASH: B256 =
    b256!("dc33d8c0ec9de14fc2c21bd6077309a0a856df22821bd092a2513426e096a789");

/// X Layer Mainnet genesis state root
///
/// The Merkle Patricia Trie root of all 1,866,483 accounts in the genesis alloc.
/// This value is hardcoded to avoid expensive computation on every startup.
pub(crate) const XLAYER_MAINNET_STATE_ROOT: B256 =
    b256!("5d335834cb1c1c20a1f44f964b16cd409aa5d10891d5c6cf26f1f2c26726efcf");

/// X Layer mainnet chain id as specified in the published `genesis.json`.
const XLAYER_MAINNET_CHAIN_ID: u64 = 196;

/// X Layer mainnet EIP-1559 parameters.
///
/// These values come from `config.optimism` in `genesis-mainnet.json`:
/// - `eip1559Denominator = 100000000`
/// - `eip1559Elasticity = 1`
///
/// They are inlined here so the built-in spec stays consistent with the full genesis.
const XLAYER_MAINNET_BASE_FEE_DENOMINATOR: u128 = 100_000_000;
const XLAYER_MAINNET_BASE_FEE_ELASTICITY: u128 = 1;

/// X Layer mainnet base fee params (same for London and Canyon forks).
const XLAYER_MAINNET_BASE_FEE_PARAMS: BaseFeeParams =
    BaseFeeParams::new(XLAYER_MAINNET_BASE_FEE_DENOMINATOR, XLAYER_MAINNET_BASE_FEE_ELASTICITY);

/// The X Layer mainnet spec
pub static XLAYER_MAINNET: LazyLock<Arc<OpChainSpec>> = LazyLock::new(|| {
    // Minimal genesis contains empty alloc field for fast loading
    let genesis = serde_json::from_str(include_str!("../res/genesis/xlayer_mainnet.json"))
        .expect("Can't deserialize X Layer Mainnet genesis json");
    let hardforks = XLAYER_MAINNET_HARDFORKS.clone();

    // Build genesis header using standard helper, then override state_root with pre-computed value
    let mut genesis_header = make_op_genesis_header(&genesis, &hardforks);
    genesis_header.state_root = XLAYER_MAINNET_STATE_ROOT;
    let genesis_header = SealedHeader::new(genesis_header, XLAYER_MAINNET_GENESIS_HASH);

    OpChainSpec {
        inner: ChainSpec {
            chain: Chain::from_id(XLAYER_MAINNET_CHAIN_ID),
            genesis_header,
            genesis,
            paris_block_and_final_difficulty: Some((0, U256::from(0))),
            hardforks,
            base_fee_params: BaseFeeParamsKind::Variable(
                vec![
                    (EthereumHardfork::London.boxed(), XLAYER_MAINNET_BASE_FEE_PARAMS),
                    (OpHardfork::Canyon.boxed(), XLAYER_MAINNET_BASE_FEE_PARAMS),
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
        serde_json::from_str(include_str!("../res/genesis/xlayer_mainnet.json"))
            .expect("Failed to parse xlayer_mainnet.json")
    }

    #[test]
    fn test_xlayer_mainnet_chain_id() {
        assert_eq!(XLAYER_MAINNET.chain().id(), 196);
    }

    #[test]
    fn test_xlayer_mainnet_genesis_hash() {
        assert_eq!(XLAYER_MAINNET.genesis_hash(), XLAYER_MAINNET_GENESIS_HASH);
    }

    #[test]
    fn test_xlayer_mainnet_state_root() {
        assert_eq!(XLAYER_MAINNET.genesis_header().state_root, XLAYER_MAINNET_STATE_ROOT);
    }

    #[test]
    fn test_xlayer_mainnet_base_fee_params() {
        assert_eq!(
            XLAYER_MAINNET.base_fee_params_at_timestamp(0),
            BaseFeeParams::new(
                XLAYER_MAINNET_BASE_FEE_DENOMINATOR,
                XLAYER_MAINNET_BASE_FEE_ELASTICITY
            )
        );
    }

    #[test]
    fn test_xlayer_mainnet_genesis_number() {
        assert_eq!(XLAYER_MAINNET.genesis_header().number, 42810021);
    }

    #[test]
    fn test_xlayer_mainnet_hardforks() {
        let spec = &*XLAYER_MAINNET;
        assert!(spec.fork(EthereumHardfork::Shanghai).active_at_timestamp(0));
        assert!(spec.fork(EthereumHardfork::Cancun).active_at_timestamp(0));
        assert!(spec.fork(OpHardfork::Bedrock).active_at_block(0));
        assert!(spec.fork(OpHardfork::Isthmus).active_at_timestamp(0));
    }

    #[test]
    fn test_xlayer_mainnet_genesis_alloc_empty() {
        assert_eq!(XLAYER_MAINNET.genesis().alloc.len(), 0);
    }

    #[test]
    fn test_xlayer_mainnet_expected_values() {
        let expected_hash =
            b256!("dc33d8c0ec9de14fc2c21bd6077309a0a856df22821bd092a2513426e096a789");
        let expected_root =
            b256!("5d335834cb1c1c20a1f44f964b16cd409aa5d10891d5c6cf26f1f2c26726efcf");
        assert_eq!(XLAYER_MAINNET_GENESIS_HASH, expected_hash);
        assert_eq!(XLAYER_MAINNET_STATE_ROOT, expected_root);
    }

    #[test]
    fn test_xlayer_mainnet_paris_activated() {
        assert_eq!(XLAYER_MAINNET.get_final_paris_total_difficulty(), Some(U256::ZERO));
    }

    #[test]
    fn test_xlayer_mainnet_canyon_base_fee_unchanged() {
        let spec = &*XLAYER_MAINNET;
        let london = spec.base_fee_params_at_timestamp(0);
        let canyon = spec.base_fee_params_at_timestamp(1);
        assert_eq!(london, canyon);
        assert_eq!(canyon, XLAYER_MAINNET_BASE_FEE_PARAMS);
    }

    #[test]
    fn test_xlayer_mainnet_genesis_header_fields() {
        let header = XLAYER_MAINNET.genesis_header();
        assert_eq!(header.withdrawals_root, Some(alloy_consensus::constants::EMPTY_WITHDRAWALS));
        assert_eq!(header.parent_beacon_block_root, Some(B256::ZERO));
        assert_eq!(header.requests_hash, Some(alloy_eips::eip7685::EMPTY_REQUESTS_HASH));
    }

    #[test]
    fn test_xlayer_mainnet_all_hardforks_active() {
        let spec = &*XLAYER_MAINNET;
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
    fn test_xlayer_mainnet_constants_match_spec() {
        assert_eq!(XLAYER_MAINNET.chain().id(), XLAYER_MAINNET_CHAIN_ID);
        assert_eq!(
            XLAYER_MAINNET.base_fee_params_at_timestamp(0),
            BaseFeeParams::new(
                XLAYER_MAINNET_BASE_FEE_DENOMINATOR,
                XLAYER_MAINNET_BASE_FEE_ELASTICITY
            )
        );
    }

    #[test]
    fn test_xlayer_mainnet_json_config_consistency() {
        let genesis = parse_genesis();
        assert_eq!(genesis.config.chain_id, XLAYER_MAINNET_CHAIN_ID);
        assert_eq!(genesis.number, Some(42810021));
        assert_eq!(genesis.timestamp, 0x68ff9031);
        assert_eq!(genesis.extra_data.as_ref(), hex!("00000000fa00000006").as_ref());
        assert_eq!(genesis.gas_limit, 0x2faf080);
        assert_eq!(genesis.difficulty, U256::ZERO);
        assert_eq!(genesis.nonce, 0);
        assert_eq!(genesis.mix_hash, B256::ZERO);
        assert_eq!(genesis.coinbase.to_string(), "0x4200000000000000000000000000000000000011");
        assert_eq!(
            genesis.parent_hash,
            Some(b256!("651b8e585c6af2eab2b0aeecb3996e2560f7fafad9892a164706c9c560d795aa"))
        );
        assert_eq!(genesis.base_fee_per_gas.map(|fee| fee as u64), Some(0x5fc01c5u64));
        assert_eq!(genesis.excess_blob_gas, Some(0));
        assert_eq!(genesis.blob_gas_used, Some(0));
    }

    #[test]
    fn test_xlayer_mainnet_json_optimism_config() {
        let genesis = parse_genesis();
        let cfg = genesis.config.extra_fields.get("optimism").expect("optimism config must exist");
        assert_eq!(
            cfg.get("eip1559Elasticity").and_then(|v| v.as_u64()).unwrap() as u128,
            XLAYER_MAINNET_BASE_FEE_ELASTICITY
        );
        assert_eq!(
            cfg.get("eip1559Denominator").and_then(|v| v.as_u64()).unwrap() as u128,
            XLAYER_MAINNET_BASE_FEE_DENOMINATOR
        );
        assert_eq!(
            cfg.get("eip1559DenominatorCanyon").and_then(|v| v.as_u64()).unwrap() as u128,
            XLAYER_MAINNET_BASE_FEE_DENOMINATOR
        );
    }

    #[test]
    fn test_xlayer_mainnet_json_hardforks_warning() {
        let genesis = parse_genesis();
        // WARNING: Hardfork times in JSON are overridden by XLAYER_MAINNET_HARDFORKS
        assert_eq!(
            genesis.config.extra_fields.get("legacyXLayerBlock").and_then(|v| v.as_u64()),
            Some(42810021)
        );
        assert_eq!(genesis.config.shanghai_time, Some(0));
        assert_eq!(genesis.config.cancun_time, Some(0));
    }

    #[test]
    fn test_xlayer_mainnet_genesis_header_matches_json() {
        let header = XLAYER_MAINNET.genesis_header();
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
        assert_eq!(header.state_root, XLAYER_MAINNET_STATE_ROOT);
    }
}
