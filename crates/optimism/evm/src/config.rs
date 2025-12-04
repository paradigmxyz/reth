pub use alloy_op_evm::{
    spec as revm_spec, spec_by_timestamp_after_bedrock as revm_spec_by_timestamp_after_bedrock,
};

use alloy_consensus::BlockHeader;
use op_revm::OpSpecId;
use revm::primitives::{Address, Bytes, B256};
use reth_mantle_forks::MantleHardforks;

/// Context relevant for execution of a next block w.r.t OP.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpNextBlockEnvAttributes {
    /// The timestamp of the next block.
    pub timestamp: u64,
    /// The suggested fee recipient for the next block.
    pub suggested_fee_recipient: Address,
    /// The randomness value for the next block.
    pub prev_randao: B256,
    /// Block gas limit.
    pub gas_limit: u64,
    /// The parent beacon block root.
    pub parent_beacon_block_root: Option<B256>,
    /// Encoded EIP-1559 parameters to include into block's `extra_data` field.
    pub extra_data: Bytes,
}

/// Map the latest active hardfork at the given header to a revm [`OpSpecId`].
pub fn revm_spec(chain_spec: impl MantleHardforks, header: impl BlockHeader) -> OpSpecId {
    revm_spec_by_timestamp_after_bedrock(chain_spec, header.timestamp())
}

/// Returns the revm [`OpSpecId`] at the given timestamp.
///
/// # Note
///
/// This is only intended to be used after the Bedrock, when hardforks are activated by
/// timestamp.
pub fn revm_spec_by_timestamp_after_bedrock(
    chain_spec: impl MantleHardforks,
    timestamp: u64,
) -> OpSpecId {
    if chain_spec.is_skadi_active_at_timestamp(timestamp) {
        OpSpecId::OSAKA
    } else if chain_spec.is_interop_active_at_timestamp(timestamp) {
        OpSpecId::INTEROP
    } else if chain_spec.is_isthmus_active_at_timestamp(timestamp) {
        OpSpecId::ISTHMUS
    } else if chain_spec.is_holocene_active_at_timestamp(timestamp) {
        OpSpecId::HOLOCENE
    } else if chain_spec.is_granite_active_at_timestamp(timestamp) {
        OpSpecId::GRANITE
    } else if chain_spec.is_fjord_active_at_timestamp(timestamp) {
        OpSpecId::FJORD
    } else if chain_spec.is_ecotone_active_at_timestamp(timestamp) {
        OpSpecId::ECOTONE
    } else if chain_spec.is_canyon_active_at_timestamp(timestamp) {
        OpSpecId::CANYON
    } else if chain_spec.is_regolith_active_at_timestamp(timestamp) {
        OpSpecId::REGOLITH
    } else {
        OpSpecId::BEDROCK
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::Header;
    use reth_chainspec::ChainSpecBuilder;
    use reth_optimism_chainspec::{OpChainSpec, OpChainSpecBuilder};

    #[test]
    fn test_revm_spec_by_timestamp_after_merge() {
        #[inline(always)]
        fn op_cs(f: impl FnOnce(OpChainSpecBuilder) -> OpChainSpecBuilder) -> OpChainSpec {
            let cs = ChainSpecBuilder::mainnet().chain(reth_chainspec::Chain::from_id(10)).into();
            f(cs).build()
        }
    }
}
