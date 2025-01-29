use alloy_consensus::Header;
use reth_chainspec::{ChainSpec, EthereumHardforks};
use reth_ethereum_forks::EthereumHardfork;

/// Map the latest active hardfork at the given header to a revm
/// [`SpecId`](revm_primitives::SpecId).
pub fn revm_spec(chain_spec: &ChainSpec, header: &Header) -> revm_primitives::SpecId {
    revm_spec_by_timestamp_and_block_number(chain_spec, header.timestamp, header.number)
}

/// Map the latest active hardfork at the given timestamp or block number to a revm
/// [`SpecId`](revm_primitives::SpecId).
pub fn revm_spec_by_timestamp_and_block_number(
    chain_spec: &ChainSpec,
    timestamp: u64,
    block_number: u64,
) -> revm_primitives::SpecId {
    if chain_spec
        .fork(EthereumHardfork::Osaka)
        .active_at_timestamp_or_number(timestamp, block_number)
    {
        revm_primitives::OSAKA
    } else if chain_spec
        .fork(EthereumHardfork::Prague)
        .active_at_timestamp_or_number(timestamp, block_number)
    {
        revm_primitives::PRAGUE
    } else if chain_spec
        .fork(EthereumHardfork::Cancun)
        .active_at_timestamp_or_number(timestamp, block_number)
    {
        revm_primitives::CANCUN
    } else if chain_spec
        .fork(EthereumHardfork::Shanghai)
        .active_at_timestamp_or_number(timestamp, block_number)
    {
        revm_primitives::SHANGHAI
    } else if chain_spec.is_paris_active_at_block(block_number).is_some_and(|active| active) {
        revm_primitives::MERGE
    } else if chain_spec
        .fork(EthereumHardfork::London)
        .active_at_timestamp_or_number(timestamp, block_number)
    {
        revm_primitives::LONDON
    } else if chain_spec
        .fork(EthereumHardfork::Berlin)
        .active_at_timestamp_or_number(timestamp, block_number)
    {
        revm_primitives::BERLIN
    } else if chain_spec
        .fork(EthereumHardfork::Istanbul)
        .active_at_timestamp_or_number(timestamp, block_number)
    {
        revm_primitives::ISTANBUL
    } else if chain_spec
        .fork(EthereumHardfork::Petersburg)
        .active_at_timestamp_or_number(timestamp, block_number)
    {
        revm_primitives::PETERSBURG
    } else if chain_spec
        .fork(EthereumHardfork::Byzantium)
        .active_at_timestamp_or_number(timestamp, block_number)
    {
        revm_primitives::BYZANTIUM
    } else if chain_spec
        .fork(EthereumHardfork::SpuriousDragon)
        .active_at_timestamp_or_number(timestamp, block_number)
    {
        revm_primitives::SPURIOUS_DRAGON
    } else if chain_spec
        .fork(EthereumHardfork::Tangerine)
        .active_at_timestamp_or_number(timestamp, block_number)
    {
        revm_primitives::TANGERINE
    } else if chain_spec
        .fork(EthereumHardfork::Homestead)
        .active_at_timestamp_or_number(timestamp, block_number)
    {
        revm_primitives::HOMESTEAD
    } else if chain_spec
        .fork(EthereumHardfork::Frontier)
        .active_at_timestamp_or_number(timestamp, block_number)
    {
        revm_primitives::FRONTIER
    } else {
        panic!(
            "invalid hardfork chainspec: expected at least one hardfork, got {:?}",
            chain_spec.hardforks
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::U256;
    use reth_chainspec::{ChainSpecBuilder, MAINNET};

    #[test]
    fn test_revm_spec_by_timestamp() {
        assert_eq!(
            revm_spec_by_timestamp_and_block_number(
                &ChainSpecBuilder::mainnet().cancun_activated().build(),
                0,
                0
            ),
            revm_primitives::CANCUN
        );
        assert_eq!(
            revm_spec_by_timestamp_and_block_number(
                &ChainSpecBuilder::mainnet().shanghai_activated().build(),
                0,
                0
            ),
            revm_primitives::SHANGHAI
        );
        let mainnet = ChainSpecBuilder::mainnet().build();
        assert_eq!(
            revm_spec_by_timestamp_and_block_number(&mainnet, 0, mainnet.paris_block().unwrap()),
            revm_primitives::MERGE
        );
    }

    #[test]
    fn test_to_revm_spec() {
        assert_eq!(
            revm_spec(&ChainSpecBuilder::mainnet().cancun_activated().build(), &Default::default()),
            revm_primitives::CANCUN
        );
        assert_eq!(
            revm_spec(
                &ChainSpecBuilder::mainnet().shanghai_activated().build(),
                &Default::default()
            ),
            revm_primitives::SHANGHAI
        );
        assert_eq!(
            revm_spec(&ChainSpecBuilder::mainnet().paris_activated().build(), &Default::default()),
            revm_primitives::MERGE
        );
        assert_eq!(
            revm_spec(&ChainSpecBuilder::mainnet().london_activated().build(), &Default::default()),
            revm_primitives::LONDON
        );
        assert_eq!(
            revm_spec(&ChainSpecBuilder::mainnet().berlin_activated().build(), &Default::default()),
            revm_primitives::BERLIN
        );
        assert_eq!(
            revm_spec(
                &ChainSpecBuilder::mainnet().istanbul_activated().build(),
                &Default::default()
            ),
            revm_primitives::ISTANBUL
        );
        assert_eq!(
            revm_spec(
                &ChainSpecBuilder::mainnet().petersburg_activated().build(),
                &Default::default()
            ),
            revm_primitives::PETERSBURG
        );
        assert_eq!(
            revm_spec(
                &ChainSpecBuilder::mainnet().byzantium_activated().build(),
                &Default::default()
            ),
            revm_primitives::BYZANTIUM
        );
        assert_eq!(
            revm_spec(
                &ChainSpecBuilder::mainnet().spurious_dragon_activated().build(),
                &Default::default()
            ),
            revm_primitives::SPURIOUS_DRAGON
        );
        assert_eq!(
            revm_spec(
                &ChainSpecBuilder::mainnet().tangerine_whistle_activated().build(),
                &Default::default()
            ),
            revm_primitives::TANGERINE
        );
        assert_eq!(
            revm_spec(
                &ChainSpecBuilder::mainnet().homestead_activated().build(),
                &Default::default()
            ),
            revm_primitives::HOMESTEAD
        );
        assert_eq!(
            revm_spec(
                &ChainSpecBuilder::mainnet().frontier_activated().build(),
                &Default::default()
            ),
            revm_primitives::FRONTIER
        );
    }

    #[test]
    fn test_eth_spec() {
        assert_eq!(
            revm_spec(&MAINNET, &Header { timestamp: 1710338135, ..Default::default() }),
            revm_primitives::CANCUN
        );
        assert_eq!(
            revm_spec(&MAINNET, &Header { timestamp: 1681338455, ..Default::default() }),
            revm_primitives::SHANGHAI
        );

        assert_eq!(
            revm_spec(
                &MAINNET,
                &Header { difficulty: U256::from(10_u128), number: 15537394, ..Default::default() }
            ),
            revm_primitives::MERGE
        );
        assert_eq!(
            revm_spec(&MAINNET, &Header { number: 15537394 - 10, ..Default::default() }),
            revm_primitives::LONDON
        );
        assert_eq!(
            revm_spec(&MAINNET, &Header { number: 12244000 + 10, ..Default::default() }),
            revm_primitives::BERLIN
        );
        assert_eq!(
            revm_spec(&MAINNET, &Header { number: 12244000 - 10, ..Default::default() }),
            revm_primitives::ISTANBUL
        );
        assert_eq!(
            revm_spec(&MAINNET, &Header { number: 7280000 + 10, ..Default::default() }),
            revm_primitives::PETERSBURG
        );
        assert_eq!(
            revm_spec(&MAINNET, &Header { number: 7280000 - 10, ..Default::default() }),
            revm_primitives::BYZANTIUM
        );
        assert_eq!(
            revm_spec(&MAINNET, &Header { number: 2675000 + 10, ..Default::default() }),
            revm_primitives::SPURIOUS_DRAGON
        );
        assert_eq!(
            revm_spec(&MAINNET, &Header { number: 2675000 - 10, ..Default::default() }),
            revm_primitives::TANGERINE
        );
        assert_eq!(
            revm_spec(&MAINNET, &Header { number: 1150000 + 10, ..Default::default() }),
            revm_primitives::HOMESTEAD
        );
        assert_eq!(
            revm_spec(&MAINNET, &Header { number: 1150000 - 10, ..Default::default() }),
            revm_primitives::FRONTIER
        );
    }
}
