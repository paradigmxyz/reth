//! Reth block execution/validation configuration and constants

use reth_primitives::{BlockNumber, ChainSpec, Hardfork};

/// Two ethereum worth of wei
pub const WEI_2ETH: u128 = 2000000000000000000u128;
/// Three ethereum worth of wei
pub const WEI_3ETH: u128 = 3000000000000000000u128;
/// Five ethereum worth of wei
pub const WEI_5ETH: u128 = 5000000000000000000u128;

/// return revm_spec from spec configuration.
pub fn revm_spec(chain_spec: &ChainSpec, for_block: BlockNumber) -> revm::SpecId {
    match for_block {
        b if Some(b) >= chain_spec.shanghai_block() => revm::MERGE_EOF,
        b if Some(b) >= chain_spec.paris_status().block_number() => revm::MERGE,
        b if Some(b) >= chain_spec.fork_block(Hardfork::London) => revm::LONDON,
        b if Some(b) >= chain_spec.fork_block(Hardfork::Berlin) => revm::BERLIN,
        b if Some(b) >= chain_spec.fork_block(Hardfork::Istanbul) => revm::ISTANBUL,
        b if Some(b) >= chain_spec.fork_block(Hardfork::Petersburg) => revm::PETERSBURG,
        b if Some(b) >= chain_spec.fork_block(Hardfork::Byzantium) => revm::BYZANTIUM,
        b if Some(b) >= chain_spec.fork_block(Hardfork::SpuriousDragon) => revm::SPURIOUS_DRAGON,
        b if Some(b) >= chain_spec.fork_block(Hardfork::Tangerine) => revm::TANGERINE,
        b if Some(b) >= chain_spec.fork_block(Hardfork::Homestead) => revm::HOMESTEAD,
        b if Some(b) >= chain_spec.fork_block(Hardfork::Frontier) => revm::FRONTIER,
        _ => panic!("wrong configuration"),
    }
}

#[cfg(test)]
mod tests {
    use crate::config::revm_spec;
    use reth_primitives::{ChainSpec, ChainSpecBuilder};
    #[test]
    fn test_to_revm_spec() {
        assert_eq!(
            revm_spec(&ChainSpecBuilder::mainnet().paris_activated().build(), 1),
            revm::MERGE
        );
        assert_eq!(
            revm_spec(&ChainSpecBuilder::mainnet().london_activated().build(), 1),
            revm::LONDON
        );
        assert_eq!(
            revm_spec(&ChainSpecBuilder::mainnet().berlin_activated().build(), 1),
            revm::BERLIN
        );
        assert_eq!(
            revm_spec(&ChainSpecBuilder::mainnet().istanbul_activated().build(), 1),
            revm::ISTANBUL
        );
        assert_eq!(
            revm_spec(&ChainSpecBuilder::mainnet().petersburg_activated().build(), 1),
            revm::PETERSBURG
        );
        assert_eq!(
            revm_spec(&ChainSpecBuilder::mainnet().byzantium_activated().build(), 1),
            revm::BYZANTIUM
        );
        assert_eq!(
            revm_spec(&ChainSpecBuilder::mainnet().spurious_dragon_activated().build(), 1),
            revm::SPURIOUS_DRAGON
        );
        assert_eq!(
            revm_spec(&ChainSpecBuilder::mainnet().tangerine_whistle_activated().build(), 1),
            revm::TANGERINE
        );
        assert_eq!(
            revm_spec(&ChainSpecBuilder::mainnet().homestead_activated().build(), 1),
            revm::HOMESTEAD
        );
        assert_eq!(
            revm_spec(&ChainSpecBuilder::mainnet().frontier_activated().build(), 1),
            revm::FRONTIER
        );
    }

    #[test]
    fn test_eth_spec() {
        let spec = ChainSpec::mainnet();
        assert_eq!(revm_spec(&spec, 15537394 + 10), revm::MERGE);
        assert_eq!(revm_spec(&spec, 15537394 - 10), revm::LONDON);
        assert_eq!(revm_spec(&spec, 12244000 + 10), revm::BERLIN);
        assert_eq!(revm_spec(&spec, 12244000 - 10), revm::ISTANBUL);
        assert_eq!(revm_spec(&spec, 7280000 + 10), revm::PETERSBURG);
        assert_eq!(revm_spec(&spec, 7280000 - 10), revm::BYZANTIUM);
        assert_eq!(revm_spec(&spec, 2675000 + 10), revm::SPURIOUS_DRAGON);
        assert_eq!(revm_spec(&spec, 2675000 - 10), revm::TANGERINE);
        assert_eq!(revm_spec(&spec, 1150000 + 10), revm::HOMESTEAD);
        assert_eq!(revm_spec(&spec, 1150000 - 10), revm::FRONTIER);
    }
}
