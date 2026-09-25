//! Chain specifications and payload attributes for test nodes.

use alloy_genesis::Genesis;
use alloy_primitives::{Address, B256};
use alloy_rpc_types_engine::PayloadAttributes;
use reth_chainspec::{ChainSpec, ChainSpecBuilder, EthereumHardfork, EthereumHardforks, MAINNET};
use std::sync::Arc;

/// Returns the genesis of the test chain.
///
/// The genesis funds the first 20 accounts derived from the test mnemonic, see
/// [`Wallet`](crate::wallet::Wallet).
pub fn test_genesis() -> Genesis {
    serde_json::from_str(include_str!("testsuite/assets/genesis.json"))
        .expect("failed to parse test genesis")
}

/// Returns a [`ChainSpecBuilder`] for the mainnet chain id with the [`test_genesis`] and no
/// hardforks activated.
///
/// Use this to schedule hardforks at non-genesis timestamps, otherwise prefer
/// [`test_chain_spec`].
pub fn test_chain_spec_builder() -> ChainSpecBuilder {
    ChainSpecBuilder::default().chain(MAINNET.chain).genesis(test_genesis())
}

/// Returns the test chain spec with every hardfork up to and including `fork` active at genesis.
///
/// # Panics
///
/// If `fork` can not be activated at genesis via [`ChainSpecBuilder`].
pub fn test_chain_spec(fork: EthereumHardfork) -> Arc<ChainSpec> {
    let builder = test_chain_spec_builder();
    let builder = match fork {
        EthereumHardfork::Frontier => builder.frontier_activated(),
        EthereumHardfork::Homestead => builder.homestead_activated(),
        EthereumHardfork::Dao => builder.dao_activated(),
        EthereumHardfork::Tangerine => builder.tangerine_whistle_activated(),
        EthereumHardfork::SpuriousDragon => builder.spurious_dragon_activated(),
        EthereumHardfork::Byzantium => builder.byzantium_activated(),
        EthereumHardfork::Constantinople => builder.constantinople_activated(),
        EthereumHardfork::Petersburg => builder.petersburg_activated(),
        EthereumHardfork::Istanbul => builder.istanbul_activated(),
        EthereumHardfork::MuirGlacier => builder.muirglacier_activated(),
        EthereumHardfork::Berlin => builder.berlin_activated(),
        EthereumHardfork::London => builder.london_activated(),
        EthereumHardfork::ArrowGlacier => builder.arrowglacier_activated(),
        EthereumHardfork::GrayGlacier => builder.grayglacier_activated(),
        EthereumHardfork::Paris => builder.paris_activated(),
        EthereumHardfork::Shanghai => builder.shanghai_activated(),
        EthereumHardfork::Cancun => builder.cancun_activated(),
        EthereumHardfork::Prague => builder.prague_activated(),
        EthereumHardfork::Osaka => builder.osaka_activated(),
        EthereumHardfork::Amsterdam => builder.amsterdam_activated(),
        EthereumHardfork::Bogota => builder.bogota_activated(),
        fork => panic!("{fork} can not be activated at genesis"),
    };
    Arc::new(builder.build())
}

/// Creates Ethereum [`PayloadAttributes`] that are valid for the hardforks active in `chain_spec`
/// at `timestamp`.
///
/// Withdrawals are set once Shanghai is active, the parent beacon block root once Cancun is
/// active, and a slot number once Amsterdam is active. The payload builder requires a slot number
/// for EIP-7843; tests use the timestamp as a deterministic dummy slot because the exact beacon
/// slot is irrelevant for local e2e payloads.
///
/// This is the default attributes generator of [`E2ETestSetupBuilder`](crate::E2ETestSetupBuilder),
/// so the same test works against any hardfork schedule, including forks activating mid-test.
pub fn eth_payload_attributes<C: EthereumHardforks>(
    chain_spec: &C,
    timestamp: u64,
) -> PayloadAttributes {
    PayloadAttributes {
        timestamp,
        prev_randao: B256::ZERO,
        suggested_fee_recipient: Address::ZERO,
        withdrawals: chain_spec.is_shanghai_active_at_timestamp(timestamp).then(Vec::new),
        parent_beacon_block_root: chain_spec
            .is_cancun_active_at_timestamp(timestamp)
            .then_some(B256::ZERO),
        slot_number: chain_spec.is_amsterdam_active_at_timestamp(timestamp).then_some(timestamp),
        ..Default::default()
    }
}
