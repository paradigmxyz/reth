//! Reth block execution/validation configuration and constants

use reth_primitives::{ChainSpec, ForkDiscriminant, Hardfork};

/// Two ethereum worth of wei
pub const WEI_2ETH: u128 = 2000000000000000000u128;
/// Three ethereum worth of wei
pub const WEI_3ETH: u128 = 3000000000000000000u128;
/// Five ethereum worth of wei
pub const WEI_5ETH: u128 = 5000000000000000000u128;

/// return revm_spec from spec configuration.
pub fn revm_spec(chain_spec: &ChainSpec, discriminant: ForkDiscriminant) -> revm::SpecId {
    match discriminant {
        d if chain_spec.fork_active(Hardfork::Shanghai, d) => revm::MERGE_EOF,
        d if chain_spec.fork_active(Hardfork::Paris, d) => revm::MERGE,
        d if chain_spec.fork_active(Hardfork::London, d) => revm::LONDON,
        d if chain_spec.fork_active(Hardfork::Berlin, d) => revm::BERLIN,
        d if chain_spec.fork_active(Hardfork::Istanbul, d) => revm::ISTANBUL,
        d if chain_spec.fork_active(Hardfork::Petersburg, d) => revm::PETERSBURG,
        d if chain_spec.fork_active(Hardfork::Byzantium, d) => revm::BYZANTIUM,
        d if chain_spec.fork_active(Hardfork::SpuriousDragon, d) => revm::SPURIOUS_DRAGON,
        d if chain_spec.fork_active(Hardfork::Tangerine, d) => revm::TANGERINE,
        d if chain_spec.fork_active(Hardfork::Homestead, d) => revm::HOMESTEAD,
        d if chain_spec.fork_active(Hardfork::Frontier, d) => revm::FRONTIER,
        _ => panic!("wrong configuration"),
    }
}

#[cfg(test)]
mod tests {
    use crate::config::revm_spec;
    use reth_primitives::{ChainSpecBuilder, ForkDiscriminant, MAINNET, U256};
    #[test]
    fn test_to_revm_spec() {
        let discriminant = ForkDiscriminant::new(1, U256::from(1), 1);
        assert_eq!(
            revm_spec(&ChainSpecBuilder::mainnet().shangai_activated().build(), discriminant),
            revm::MERGE_EOF
        );
        assert_eq!(
            revm_spec(&ChainSpecBuilder::mainnet().paris_activated().build(), discriminant),
            revm::MERGE
        );
        assert_eq!(
            revm_spec(&ChainSpecBuilder::mainnet().london_activated().build(), discriminant),
            revm::LONDON
        );
        assert_eq!(
            revm_spec(&ChainSpecBuilder::mainnet().berlin_activated().build(), discriminant),
            revm::BERLIN
        );
        assert_eq!(
            revm_spec(&ChainSpecBuilder::mainnet().istanbul_activated().build(), discriminant),
            revm::ISTANBUL
        );
        assert_eq!(
            revm_spec(&ChainSpecBuilder::mainnet().petersburg_activated().build(), discriminant),
            revm::PETERSBURG
        );
        assert_eq!(
            revm_spec(&ChainSpecBuilder::mainnet().byzantium_activated().build(), discriminant),
            revm::BYZANTIUM
        );
        assert_eq!(
            revm_spec(
                &ChainSpecBuilder::mainnet().spurious_dragon_activated().build(),
                discriminant
            ),
            revm::SPURIOUS_DRAGON
        );
        assert_eq!(
            revm_spec(
                &ChainSpecBuilder::mainnet().tangerine_whistle_activated().build(),
                discriminant
            ),
            revm::TANGERINE
        );
        assert_eq!(
            revm_spec(&ChainSpecBuilder::mainnet().homestead_activated().build(), discriminant),
            revm::HOMESTEAD
        );
        assert_eq!(
            revm_spec(&ChainSpecBuilder::mainnet().frontier_activated().build(), discriminant),
            revm::FRONTIER
        );
    }

    #[test]
    fn test_eth_spec() {
        let post_merge_td = MAINNET.terminal_total_difficulty().unwrap();
        let pre_merge_td = post_merge_td.saturating_sub(U256::from(10));

        assert_eq!(
            revm_spec(&MAINNET, ForkDiscriminant::new(15537394 + 10, post_merge_td, 1674477448)),
            revm::MERGE
        );
        assert_eq!(
            revm_spec(&MAINNET, ForkDiscriminant::new(15537394 - 10, pre_merge_td, 1674477448)),
            revm::LONDON
        );
        assert_eq!(
            revm_spec(&MAINNET, ForkDiscriminant::new(12244000 + 10, pre_merge_td, 1674477448)),
            revm::BERLIN
        );
        assert_eq!(
            revm_spec(&MAINNET, ForkDiscriminant::new(12244000 - 10, pre_merge_td, 1674477448)),
            revm::ISTANBUL
        );
        assert_eq!(
            revm_spec(&MAINNET, ForkDiscriminant::new(7280000 + 10, pre_merge_td, 1674477448)),
            revm::PETERSBURG
        );
        assert_eq!(
            revm_spec(&MAINNET, ForkDiscriminant::new(7280000 - 10, pre_merge_td, 1674477448)),
            revm::BYZANTIUM
        );
        assert_eq!(
            revm_spec(&MAINNET, ForkDiscriminant::new(2675000 + 10, pre_merge_td, 1674477448)),
            revm::SPURIOUS_DRAGON
        );
        assert_eq!(
            revm_spec(&MAINNET, ForkDiscriminant::new(2675000 - 10, pre_merge_td, 1674477448)),
            revm::TANGERINE
        );
        assert_eq!(
            revm_spec(&MAINNET, ForkDiscriminant::new(1150000 + 10, pre_merge_td, 1674477448)),
            revm::HOMESTEAD
        );
        assert_eq!(
            revm_spec(&MAINNET, ForkDiscriminant::new(1150000 - 10, pre_merge_td, 1674477448)),
            revm::FRONTIER
        );
    }
}
