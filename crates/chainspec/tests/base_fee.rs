use alloy_consensus::Header;
use alloy_eips::eip1559::INITIAL_BASE_FEE;
use reth_chainspec::{ChainSpec, EthChainSpec};

fn parent_header() -> Header {
    Header {
        gas_used: 15_000_000,
        gas_limit: 30_000_000,
        base_fee_per_gas: Some(INITIAL_BASE_FEE),
        timestamp: 1_000,
        ..Default::default()
    }
}

#[test]
fn default_chain_spec_base_fee_matches_formula() {
    let spec = ChainSpec::default();
    let parent = parent_header();

    // For testing, assume next block has timestamp 12 seconds later
    let next_timestamp = parent.timestamp + 12;

    let expected = parent
        .next_block_base_fee(spec.base_fee_params_at_timestamp(next_timestamp))
        .unwrap_or_default();

    let got = spec.next_block_base_fee(&parent, next_timestamp);
    assert_eq!(expected, got, "Base fee calculation does not match expected value");
}
