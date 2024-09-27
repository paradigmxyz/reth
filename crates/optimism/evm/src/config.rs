use reth_chainspec::ChainSpec;
use reth_ethereum_forks::{EthereumHardfork, Head};
use reth_optimism_forks::OptimismHardfork;

/// Returns the revm [`SpecId`](revm_primitives::SpecId) at the given timestamp.
///
/// # Note
///
/// This is only intended to be used after the Bedrock, when hardforks are activated by
/// timestamp.
pub fn revm_spec_by_timestamp_after_bedrock(
    chain_spec: &ChainSpec,
    timestamp: u64,
) -> revm_primitives::SpecId {
    if chain_spec.fork(OptimismHardfork::Granite).active_at_timestamp(timestamp) {
        revm_primitives::GRANITE
    } else if chain_spec.fork(OptimismHardfork::Fjord).active_at_timestamp(timestamp) {
        revm_primitives::FJORD
    } else if chain_spec.fork(OptimismHardfork::Ecotone).active_at_timestamp(timestamp) {
        revm_primitives::ECOTONE
    } else if chain_spec.fork(OptimismHardfork::Canyon).active_at_timestamp(timestamp) {
        revm_primitives::CANYON
    } else if chain_spec.fork(OptimismHardfork::Regolith).active_at_timestamp(timestamp) {
        revm_primitives::REGOLITH
    } else {
        revm_primitives::BEDROCK
    }
}

/// Map the latest active hardfork at the given block to a revm [`SpecId`](revm_primitives::SpecId).
pub fn revm_spec(chain_spec: &ChainSpec, block: &Head) -> revm_primitives::SpecId {
    if chain_spec.fork(OptimismHardfork::Granite).active_at_head(block) {
        revm_primitives::GRANITE
    } else if chain_spec.fork(OptimismHardfork::Fjord).active_at_head(block) {
        revm_primitives::FJORD
    } else if chain_spec.fork(OptimismHardfork::Ecotone).active_at_head(block) {
        revm_primitives::ECOTONE
    } else if chain_spec.fork(OptimismHardfork::Canyon).active_at_head(block) {
        revm_primitives::CANYON
    } else if chain_spec.fork(OptimismHardfork::Regolith).active_at_head(block) {
        revm_primitives::REGOLITH
    } else if chain_spec.fork(OptimismHardfork::Bedrock).active_at_head(block) {
        revm_primitives::BEDROCK
    } else if chain_spec.fork(EthereumHardfork::Prague).active_at_head(block) {
        revm_primitives::PRAGUE
    } else if chain_spec.fork(EthereumHardfork::Cancun).active_at_head(block) {
        revm_primitives::CANCUN
    } else if chain_spec.fork(EthereumHardfork::Shanghai).active_at_head(block) {
        revm_primitives::SHANGHAI
    } else if chain_spec.fork(EthereumHardfork::Paris).active_at_head(block) {
        revm_primitives::MERGE
    } else if chain_spec.fork(EthereumHardfork::London).active_at_head(block) {
        revm_primitives::LONDON
    } else if chain_spec.fork(EthereumHardfork::Berlin).active_at_head(block) {
        revm_primitives::BERLIN
    } else if chain_spec.fork(EthereumHardfork::Istanbul).active_at_head(block) {
        revm_primitives::ISTANBUL
    } else if chain_spec.fork(EthereumHardfork::Petersburg).active_at_head(block) {
        revm_primitives::PETERSBURG
    } else if chain_spec.fork(EthereumHardfork::Byzantium).active_at_head(block) {
        revm_primitives::BYZANTIUM
    } else if chain_spec.fork(EthereumHardfork::SpuriousDragon).active_at_head(block) {
        revm_primitives::SPURIOUS_DRAGON
    } else if chain_spec.fork(EthereumHardfork::Tangerine).active_at_head(block) {
        revm_primitives::TANGERINE
    } else if chain_spec.fork(EthereumHardfork::Homestead).active_at_head(block) {
        revm_primitives::HOMESTEAD
    } else if chain_spec.fork(EthereumHardfork::Frontier).active_at_head(block) {
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
    use reth_chainspec::ChainSpecBuilder;

    #[test]
    fn test_revm_spec_by_timestamp_after_merge() {
        #[inline(always)]
        fn op_cs(f: impl FnOnce(ChainSpecBuilder) -> ChainSpecBuilder) -> ChainSpec {
            let cs = ChainSpecBuilder::mainnet().chain(reth_chainspec::Chain::from_id(10));
            f(cs).build()
        }
        assert_eq!(
            revm_spec_by_timestamp_after_bedrock(&op_cs(|cs| cs.granite_activated()), 0),
            revm_primitives::GRANITE
        );
        assert_eq!(
            revm_spec_by_timestamp_after_bedrock(&op_cs(|cs| cs.fjord_activated()), 0),
            revm_primitives::FJORD
        );
        assert_eq!(
            revm_spec_by_timestamp_after_bedrock(&op_cs(|cs| cs.ecotone_activated()), 0),
            revm_primitives::ECOTONE
        );
        assert_eq!(
            revm_spec_by_timestamp_after_bedrock(&op_cs(|cs| cs.canyon_activated()), 0),
            revm_primitives::CANYON
        );
        assert_eq!(
            revm_spec_by_timestamp_after_bedrock(&op_cs(|cs| cs.bedrock_activated()), 0),
            revm_primitives::BEDROCK
        );
        assert_eq!(
            revm_spec_by_timestamp_after_bedrock(&op_cs(|cs| cs.regolith_activated()), 0),
            revm_primitives::REGOLITH
        );
    }

    #[test]
    fn test_to_revm_spec() {
        #[inline(always)]
        fn op_cs(f: impl FnOnce(ChainSpecBuilder) -> ChainSpecBuilder) -> ChainSpec {
            let cs = ChainSpecBuilder::mainnet().chain(reth_chainspec::Chain::from_id(10));
            f(cs).build()
        }
        assert_eq!(
            revm_spec(&op_cs(|cs| cs.granite_activated()), &Head::default()),
            revm_primitives::GRANITE
        );
        assert_eq!(
            revm_spec(&op_cs(|cs| cs.fjord_activated()), &Head::default()),
            revm_primitives::FJORD
        );
        assert_eq!(
            revm_spec(&op_cs(|cs| cs.ecotone_activated()), &Head::default()),
            revm_primitives::ECOTONE
        );
        assert_eq!(
            revm_spec(&op_cs(|cs| cs.canyon_activated()), &Head::default()),
            revm_primitives::CANYON
        );
        assert_eq!(
            revm_spec(&op_cs(|cs| cs.bedrock_activated()), &Head::default()),
            revm_primitives::BEDROCK
        );
        assert_eq!(
            revm_spec(&op_cs(|cs| cs.regolith_activated()), &Head::default()),
            revm_primitives::REGOLITH
        );
    }
}
