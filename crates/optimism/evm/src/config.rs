use alloy_consensus::Header;
use reth_optimism_chainspec::OpChainSpec;
use reth_optimism_forks::OpHardfork;

/// Map the latest active hardfork at the given header to a revm
/// [`SpecId`](revm_primitives::SpecId).
pub fn revm_spec(chain_spec: &OpChainSpec, header: &Header) -> revm_primitives::SpecId {
    revm_spec_by_timestamp_after_bedrock(chain_spec, header.timestamp)
}

/// Returns the revm [`SpecId`](revm_primitives::SpecId) at the given timestamp.
///
/// # Note
///
/// This is only intended to be used after the Bedrock, when hardforks are activated by
/// timestamp.
pub fn revm_spec_by_timestamp_after_bedrock(
    chain_spec: &OpChainSpec,
    timestamp: u64,
) -> revm_primitives::SpecId {
    if chain_spec.fork(OpHardfork::Isthmus).active_at_timestamp(timestamp) {
        todo!()
    } else if chain_spec.fork(OpHardfork::Holocene).active_at_timestamp(timestamp) {
        revm_primitives::HOLOCENE
    } else if chain_spec.fork(OpHardfork::Granite).active_at_timestamp(timestamp) {
        revm_primitives::GRANITE
    } else if chain_spec.fork(OpHardfork::Fjord).active_at_timestamp(timestamp) {
        revm_primitives::FJORD
    } else if chain_spec.fork(OpHardfork::Ecotone).active_at_timestamp(timestamp) {
        revm_primitives::ECOTONE
    } else if chain_spec.fork(OpHardfork::Canyon).active_at_timestamp(timestamp) {
        revm_primitives::CANYON
    } else if chain_spec.fork(OpHardfork::Regolith).active_at_timestamp(timestamp) {
        revm_primitives::REGOLITH
    } else {
        revm_primitives::BEDROCK
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_chainspec::ChainSpecBuilder;
    use reth_optimism_chainspec::{OpChainSpec, OpChainSpecBuilder};

    #[test]
    fn test_revm_spec_by_timestamp_after_merge() {
        #[inline(always)]
        fn op_cs(f: impl FnOnce(OpChainSpecBuilder) -> OpChainSpecBuilder) -> OpChainSpec {
            let cs = ChainSpecBuilder::mainnet().chain(reth_chainspec::Chain::from_id(10)).into();
            f(cs).build()
        }
        assert_eq!(
            revm_spec_by_timestamp_after_bedrock(&op_cs(|cs| cs.holocene_activated()), 0),
            revm_primitives::HOLOCENE
        );
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
        fn op_cs(f: impl FnOnce(OpChainSpecBuilder) -> OpChainSpecBuilder) -> OpChainSpec {
            let cs = ChainSpecBuilder::mainnet().chain(reth_chainspec::Chain::from_id(10)).into();
            f(cs).build()
        }
        assert_eq!(
            revm_spec(&op_cs(|cs| cs.holocene_activated()), &Default::default()),
            revm_primitives::HOLOCENE
        );
        assert_eq!(
            revm_spec(&op_cs(|cs| cs.granite_activated()), &Default::default()),
            revm_primitives::GRANITE
        );
        assert_eq!(
            revm_spec(&op_cs(|cs| cs.fjord_activated()), &Default::default()),
            revm_primitives::FJORD
        );
        assert_eq!(
            revm_spec(&op_cs(|cs| cs.ecotone_activated()), &Default::default()),
            revm_primitives::ECOTONE
        );
        assert_eq!(
            revm_spec(&op_cs(|cs| cs.canyon_activated()), &Default::default()),
            revm_primitives::CANYON
        );
        assert_eq!(
            revm_spec(&op_cs(|cs| cs.bedrock_activated()), &Default::default()),
            revm_primitives::BEDROCK
        );
        assert_eq!(
            revm_spec(&op_cs(|cs| cs.regolith_activated()), &Default::default()),
            revm_primitives::REGOLITH
        );
    }
}
