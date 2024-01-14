use crate::{revm_primitives, ChainSpec, Hardfork, Head};

/// Returns the spec id at the given timestamp.
///
/// Note: This is only intended to be used after the merge, when hardforks are activated by
/// timestamp.
pub fn revm_spec_by_timestamp_after_merge(
    chain_spec: &ChainSpec,
    timestamp: u64,
) -> revm_primitives::SpecId {
    #[cfg(feature = "optimism")]
    if chain_spec.is_optimism() {
        if chain_spec.fork(Hardfork::Canyon).active_at_timestamp(timestamp) {
            return revm_primitives::CANYON
        } else if chain_spec.fork(Hardfork::Regolith).active_at_timestamp(timestamp) {
            return revm_primitives::REGOLITH
        } else {
            return revm_primitives::BEDROCK
        }
    }

    if chain_spec.is_cancun_active_at_timestamp(timestamp) {
        revm_primitives::CANCUN
    } else if chain_spec.is_shanghai_active_at_timestamp(timestamp) {
        revm_primitives::SHANGHAI
    } else {
        revm_primitives::MERGE
    }
}

/// return revm_spec from spec configuration.
pub fn revm_spec(chain_spec: &ChainSpec, block: Head) -> revm_primitives::SpecId {
    #[cfg(feature = "optimism")]
    if chain_spec.is_optimism() {
        if chain_spec.fork(Hardfork::Canyon).active_at_head(&block) {
            return revm_primitives::CANYON
        } else if chain_spec.fork(Hardfork::Regolith).active_at_head(&block) {
            return revm_primitives::REGOLITH
        } else if chain_spec.fork(Hardfork::Bedrock).active_at_head(&block) {
            return revm_primitives::BEDROCK
        }
    }

    if chain_spec.fork(Hardfork::Cancun).active_at_head(&block) {
        revm_primitives::CANCUN
    } else if chain_spec.fork(Hardfork::Shanghai).active_at_head(&block) {
        revm_primitives::SHANGHAI
    } else if chain_spec.fork(Hardfork::Paris).active_at_head(&block) {
        revm_primitives::MERGE
    } else if chain_spec.fork(Hardfork::London).active_at_head(&block) {
        revm_primitives::LONDON
    } else if chain_spec.fork(Hardfork::Berlin).active_at_head(&block) {
        revm_primitives::BERLIN
    } else if chain_spec.fork(Hardfork::Istanbul).active_at_head(&block) {
        revm_primitives::ISTANBUL
    } else if chain_spec.fork(Hardfork::Petersburg).active_at_head(&block) {
        revm_primitives::PETERSBURG
    } else if chain_spec.fork(Hardfork::Byzantium).active_at_head(&block) {
        revm_primitives::BYZANTIUM
    } else if chain_spec.fork(Hardfork::SpuriousDragon).active_at_head(&block) {
        revm_primitives::SPURIOUS_DRAGON
    } else if chain_spec.fork(Hardfork::Tangerine).active_at_head(&block) {
        revm_primitives::TANGERINE
    } else if chain_spec.fork(Hardfork::Homestead).active_at_head(&block) {
        revm_primitives::HOMESTEAD
    } else if chain_spec.fork(Hardfork::Frontier).active_at_head(&block) {
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
    use crate::{ChainSpecBuilder, Head, MAINNET, U256};

    #[test]
    fn test_to_revm_spec() {
        assert_eq!(
            revm_spec(&ChainSpecBuilder::mainnet().cancun_activated().build(), Head::default()),
            revm_primitives::CANCUN
        );
        assert_eq!(
            revm_spec(&ChainSpecBuilder::mainnet().shanghai_activated().build(), Head::default()),
            revm_primitives::SHANGHAI
        );
        assert_eq!(
            revm_spec(&ChainSpecBuilder::mainnet().paris_activated().build(), Head::default()),
            revm_primitives::MERGE
        );
        assert_eq!(
            revm_spec(&ChainSpecBuilder::mainnet().london_activated().build(), Head::default()),
            revm_primitives::LONDON
        );
        assert_eq!(
            revm_spec(&ChainSpecBuilder::mainnet().berlin_activated().build(), Head::default()),
            revm_primitives::BERLIN
        );
        assert_eq!(
            revm_spec(&ChainSpecBuilder::mainnet().istanbul_activated().build(), Head::default()),
            revm_primitives::ISTANBUL
        );
        assert_eq!(
            revm_spec(&ChainSpecBuilder::mainnet().petersburg_activated().build(), Head::default()),
            revm_primitives::PETERSBURG
        );
        assert_eq!(
            revm_spec(&ChainSpecBuilder::mainnet().byzantium_activated().build(), Head::default()),
            revm_primitives::BYZANTIUM
        );
        assert_eq!(
            revm_spec(
                &ChainSpecBuilder::mainnet().spurious_dragon_activated().build(),
                Head::default()
            ),
            revm_primitives::SPURIOUS_DRAGON
        );
        assert_eq!(
            revm_spec(
                &ChainSpecBuilder::mainnet().tangerine_whistle_activated().build(),
                Head::default()
            ),
            revm_primitives::TANGERINE
        );
        assert_eq!(
            revm_spec(&ChainSpecBuilder::mainnet().homestead_activated().build(), Head::default()),
            revm_primitives::HOMESTEAD
        );
        assert_eq!(
            revm_spec(&ChainSpecBuilder::mainnet().frontier_activated().build(), Head::default()),
            revm_primitives::FRONTIER
        );
        #[cfg(feature = "optimism")]
        {
            #[inline(always)]
            fn op_cs(f: impl FnOnce(ChainSpecBuilder) -> ChainSpecBuilder) -> ChainSpec {
                let cs = ChainSpecBuilder::mainnet().chain(alloy_chains::Chain::from_id(10));
                f(cs).build()
            }

            assert_eq!(
                revm_spec(&op_cs(|cs| cs.canyon_activated()), Head::default()),
                revm_primitives::CANYON
            );
            assert_eq!(
                revm_spec(&op_cs(|cs| cs.bedrock_activated()), Head::default()),
                revm_primitives::BEDROCK
            );
            assert_eq!(
                revm_spec(&op_cs(|cs| cs.regolith_activated()), Head::default()),
                revm_primitives::REGOLITH
            );
        }
    }

    #[test]
    fn test_eth_spec() {
        assert_eq!(
            revm_spec(
                &MAINNET,
                Head {
                    total_difficulty: U256::from(58_750_000_000_000_000_000_010_u128),
                    difficulty: U256::from(10_u128),
                    ..Default::default()
                }
            ),
            revm_primitives::MERGE
        );
        // TTD trumps the block number
        assert_eq!(
            revm_spec(
                &MAINNET,
                Head {
                    number: 15537394 - 10,
                    total_difficulty: U256::from(58_750_000_000_000_000_000_010_u128),
                    difficulty: U256::from(10_u128),
                    ..Default::default()
                }
            ),
            revm_primitives::MERGE
        );
        assert_eq!(
            revm_spec(&MAINNET, Head { number: 15537394 - 10, ..Default::default() }),
            revm_primitives::LONDON
        );
        assert_eq!(
            revm_spec(&MAINNET, Head { number: 12244000 + 10, ..Default::default() }),
            revm_primitives::BERLIN
        );
        assert_eq!(
            revm_spec(&MAINNET, Head { number: 12244000 - 10, ..Default::default() }),
            revm_primitives::ISTANBUL
        );
        assert_eq!(
            revm_spec(&MAINNET, Head { number: 7280000 + 10, ..Default::default() }),
            revm_primitives::PETERSBURG
        );
        assert_eq!(
            revm_spec(&MAINNET, Head { number: 7280000 - 10, ..Default::default() }),
            revm_primitives::BYZANTIUM
        );
        assert_eq!(
            revm_spec(&MAINNET, Head { number: 2675000 + 10, ..Default::default() }),
            revm_primitives::SPURIOUS_DRAGON
        );
        assert_eq!(
            revm_spec(&MAINNET, Head { number: 2675000 - 10, ..Default::default() }),
            revm_primitives::TANGERINE
        );
        assert_eq!(
            revm_spec(&MAINNET, Head { number: 1150000 + 10, ..Default::default() }),
            revm_primitives::HOMESTEAD
        );
        assert_eq!(
            revm_spec(&MAINNET, Head { number: 1150000 - 10, ..Default::default() }),
            revm_primitives::FRONTIER
        );
    }
}
