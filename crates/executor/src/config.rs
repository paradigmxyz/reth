//! Reth block execution/validation configuration and constants

use reth_primitives::{ChainSpec, Hardfork, Head};

/// Two ethereum worth of wei
pub const WEI_2ETH: u128 = 2000000000000000000u128;
/// Three ethereum worth of wei
pub const WEI_3ETH: u128 = 3000000000000000000u128;
/// Five ethereum worth of wei
pub const WEI_5ETH: u128 = 5000000000000000000u128;

/// return revm_spec from spec configuration.
pub fn revm_spec(chain_spec: &ChainSpec, block: Head) -> revm::SpecId {
    if chain_spec.fork(Hardfork::Shanghai).active_at_head(&block) {
        revm::MERGE_EOF
    } else if chain_spec.fork(Hardfork::Paris).active_at_head(&block) {
        revm::MERGE
    } else if chain_spec.fork(Hardfork::London).active_at_head(&block) {
        revm::LONDON
    } else if chain_spec.fork(Hardfork::Berlin).active_at_head(&block) {
        revm::BERLIN
    } else if chain_spec.fork(Hardfork::Istanbul).active_at_head(&block) {
        revm::ISTANBUL
    } else if chain_spec.fork(Hardfork::Petersburg).active_at_head(&block) {
        revm::PETERSBURG
    } else if chain_spec.fork(Hardfork::Byzantium).active_at_head(&block) {
        revm::BYZANTIUM
    } else if chain_spec.fork(Hardfork::SpuriousDragon).active_at_head(&block) {
        revm::SPURIOUS_DRAGON
    } else if chain_spec.fork(Hardfork::Tangerine).active_at_head(&block) {
        revm::TANGERINE
    } else if chain_spec.fork(Hardfork::Homestead).active_at_head(&block) {
        revm::HOMESTEAD
    } else if chain_spec.fork(Hardfork::Frontier).active_at_head(&block) {
        revm::FRONTIER
    } else {
        panic!("wrong configuration")
    }
}

#[cfg(test)]
mod tests {
    use crate::config::revm_spec;
    use reth_primitives::{ChainSpecBuilder, Head, MAINNET, U256};
    #[test]
    fn test_to_revm_spec() {
        assert_eq!(
            revm_spec(&ChainSpecBuilder::mainnet().paris_activated().build(), Head::default()),
            revm::MERGE
        );
        assert_eq!(
            revm_spec(&ChainSpecBuilder::mainnet().london_activated().build(), Head::default()),
            revm::LONDON
        );
        assert_eq!(
            revm_spec(&ChainSpecBuilder::mainnet().berlin_activated().build(), Head::default()),
            revm::BERLIN
        );
        assert_eq!(
            revm_spec(&ChainSpecBuilder::mainnet().istanbul_activated().build(), Head::default()),
            revm::ISTANBUL
        );
        assert_eq!(
            revm_spec(&ChainSpecBuilder::mainnet().petersburg_activated().build(), Head::default()),
            revm::PETERSBURG
        );
        assert_eq!(
            revm_spec(&ChainSpecBuilder::mainnet().byzantium_activated().build(), Head::default()),
            revm::BYZANTIUM
        );
        assert_eq!(
            revm_spec(
                &ChainSpecBuilder::mainnet().spurious_dragon_activated().build(),
                Head::default()
            ),
            revm::SPURIOUS_DRAGON
        );
        assert_eq!(
            revm_spec(
                &ChainSpecBuilder::mainnet().tangerine_whistle_activated().build(),
                Head::default()
            ),
            revm::TANGERINE
        );
        assert_eq!(
            revm_spec(&ChainSpecBuilder::mainnet().homestead_activated().build(), Head::default()),
            revm::HOMESTEAD
        );
        assert_eq!(
            revm_spec(&ChainSpecBuilder::mainnet().frontier_activated().build(), Head::default()),
            revm::FRONTIER
        );
    }

    #[test]
    fn test_eth_spec() {
        assert_eq!(
            revm_spec(
                &MAINNET,
                Head {
                    total_difficulty: U256::from(58_750_000_000_000_000_000_000u128),
                    ..Default::default()
                }
            ),
            revm::MERGE
        );
        // TTD trumps the block number
        assert_eq!(
            revm_spec(
                &MAINNET,
                Head {
                    number: 15537394 - 10,
                    total_difficulty: U256::from(58_750_000_000_000_000_000_000u128),
                    ..Default::default()
                }
            ),
            revm::MERGE
        );
        assert_eq!(
            revm_spec(&MAINNET, Head { number: 15537394 - 10, ..Default::default() }),
            revm::LONDON
        );
        assert_eq!(
            revm_spec(&MAINNET, Head { number: 12244000 + 10, ..Default::default() }),
            revm::BERLIN
        );
        assert_eq!(
            revm_spec(&MAINNET, Head { number: 12244000 - 10, ..Default::default() }),
            revm::ISTANBUL
        );
        assert_eq!(
            revm_spec(&MAINNET, Head { number: 7280000 + 10, ..Default::default() }),
            revm::PETERSBURG
        );
        assert_eq!(
            revm_spec(&MAINNET, Head { number: 7280000 - 10, ..Default::default() }),
            revm::BYZANTIUM
        );
        assert_eq!(
            revm_spec(&MAINNET, Head { number: 2675000 + 10, ..Default::default() }),
            revm::SPURIOUS_DRAGON
        );
        assert_eq!(
            revm_spec(&MAINNET, Head { number: 2675000 - 10, ..Default::default() }),
            revm::TANGERINE
        );
        assert_eq!(
            revm_spec(&MAINNET, Head { number: 1150000 + 10, ..Default::default() }),
            revm::HOMESTEAD
        );
        assert_eq!(
            revm_spec(&MAINNET, Head { number: 1150000 - 10, ..Default::default() }),
            revm::FRONTIER
        );
    }
}
