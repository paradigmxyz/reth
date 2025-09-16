//! Base fee related utilities for Optimism chains.

use alloy_consensus::BlockHeader;
use op_alloy_consensus::{
    decode_holocene_extra_data, decode_min_base_fee_extra_data, EIP1559ParamError,
};
use reth_chainspec::{BaseFeeParams, EthChainSpec};
use reth_optimism_forks::OpHardforks;

/// Extracts the Holocene 1599 parameters from the encoded extra data from the parent header.
///
/// Caution: Caller must ensure that holocene is active in the parent header.
///
/// See also [Base fee computation](https://github.com/ethereum-optimism/specs/blob/main/specs/protocol/holocene/exec-engine.md#base-fee-computation)
pub fn decode_holocene_base_fee<H>(
    chain_spec: impl EthChainSpec + OpHardforks,
    parent: &H,
    timestamp: u64,
) -> Result<u64, EIP1559ParamError>
where
    H: BlockHeader,
{
    let (elasticity, denominator) = decode_holocene_extra_data(parent.extra_data())?;
    let base_fee_params = if elasticity == 0 && denominator == 0 {
        chain_spec.base_fee_params_at_timestamp(timestamp)
    } else {
        BaseFeeParams::new(denominator as u128, elasticity as u128)
    };

    Ok(parent.next_block_base_fee(base_fee_params).unwrap_or_default())
}

pub fn decode_min_base_fee<H>(
    chain_spec: impl EthChainSpec + OpHardforks,
    parent: &H,
    timestamp: u64,
) -> Result<u64, EIP1559ParamError>
where
    H: BlockHeader,
{
    // TODO: function is in op-alloy
    let (elasticity, denominator, significand, exponent) =
        decode_min_base_fee_extra_data(parent.extra_data())?;
    let base_fee_params = if elasticity == 0 && denominator == 0 {
        chain_spec.base_fee_params_at_timestamp(timestamp)
    } else {
        BaseFeeParams::new(denominator as u128, elasticity as u128)
    };

    let base_fee = parent.next_block_base_fee(base_fee_params).unwrap_or_default();
    if chain_spec.is_jovian_active_at_block(timestamp) {
        let min_base_fee = significand.checked_mul(10_u128.pow(exponent)).unwrap_or(0);
        if base_fee < min_base_fee {
            return Ok(min_base_fee);
        }
    }
    Ok(base_fee)
}
