//! Reth block execution/validation configuration and constants

use reth_primitives::{chains::ChainSpec, BlockNumber, Hardfork};

/// Two ethereum worth of wei
pub const WEI_2ETH: u128 = 2000000000000000000u128;
/// Three ethereum worth of wei
pub const WEI_3ETH: u128 = 3000000000000000000u128;
/// Five ethereum worth of wei
pub const WEI_5ETH: u128 = 5000000000000000000u128;

/// return revm_spec from spec configuration.
pub fn revm_spec<CS: ChainSpec>(chain_spec: &CS, for_block: BlockNumber) -> revm::SpecId {
    match for_block {
        b if b >= chain_spec.shanghai_block() => revm::MERGE_EOF,
        b if b >= chain_spec.paris_block() => revm::MERGE,
        b if b >= chain_spec.fork_block(Hardfork::London) => revm::LONDON,
        b if b >= chain_spec.fork_block(Hardfork::Berlin) => revm::BERLIN,
        b if b >= chain_spec.fork_block(Hardfork::Istanbul) => revm::ISTANBUL,
        b if b >= chain_spec.fork_block(Hardfork::Petersburg) => revm::PETERSBURG,
        b if b >= chain_spec.fork_block(Hardfork::Byzantium) => revm::BYZANTIUM,
        b if b >= chain_spec.fork_block(Hardfork::SpuriousDragon) => revm::SPURIOUS_DRAGON,
        b if b >= chain_spec.fork_block(Hardfork::Tangerine) => revm::TANGERINE,
        b if b >= chain_spec.fork_block(Hardfork::Homestead) => revm::HOMESTEAD,
        b if b >= chain_spec.fork_block(Hardfork::Frontier) => revm::FRONTIER,
        _ => panic!("wrong configuration"),
    }
}

#[cfg(test)]
mod tests {
    use crate::config::revm_spec;
    use reth_primitives::chains::{ChainSpecUnified, MainnetSpec};
    #[test]
    fn test_to_revm_spec() {
        assert_eq!(
            revm_spec(&ChainSpecUnified::Mainnet.into_customized().paris_activated().build(), 1),
            revm::MERGE
        );
        assert_eq!(
            revm_spec(&ChainSpecUnified::Mainnet.into_customized().london_activated().build(), 1),
            revm::LONDON
        );
        assert_eq!(
            revm_spec(&ChainSpecUnified::Mainnet.into_customized().berlin_activated().build(), 1),
            revm::BERLIN
        );
        assert_eq!(
            revm_spec(&ChainSpecUnified::Mainnet.into_customized().istanbul_activated().build(), 1),
            revm::ISTANBUL
        );
        assert_eq!(
            revm_spec(
                &ChainSpecUnified::Mainnet.into_customized().petersburg_activated().build(),
                1
            ),
            revm::PETERSBURG
        );
        assert_eq!(
            revm_spec(
                &ChainSpecUnified::Mainnet.into_customized().byzantium_activated().build(),
                1
            ),
            revm::BYZANTIUM
        );
        assert_eq!(
            revm_spec(
                &ChainSpecUnified::Mainnet.into_customized().spurious_dragon_activated().build(),
                1
            ),
            revm::SPURIOUS_DRAGON
        );
        assert_eq!(
            revm_spec(
                &ChainSpecUnified::Mainnet.into_customized().tangerine_whistle_activated().build(),
                1
            ),
            revm::TANGERINE
        );
        assert_eq!(
            revm_spec(
                &ChainSpecUnified::Mainnet.into_customized().homestead_activated().build(),
                1
            ),
            revm::HOMESTEAD
        );
        assert_eq!(
            revm_spec(&ChainSpecUnified::Mainnet.into_customized().frontier_activated().build(), 1),
            revm::FRONTIER
        );
    }

    #[test]
    fn test_eth_spec() {
        let spec = MainnetSpec::default();
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
