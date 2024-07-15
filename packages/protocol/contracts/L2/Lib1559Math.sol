// SPDX-License-Identifier: MIT
pragma solidity 0.8.24;

import "../thirdparty/solmate/LibFixedPointMath.sol";
import "../libs/LibMath.sol";

/// @title Lib1559Math
/// @notice Implements e^(x) based bonding curve for EIP-1559
/// @dev See https://ethresear.ch/t/make-eip-1559-more-like-an-amm-curve/9082 but some minor
/// difference as stated in docs/eip1559_on_l2.md.
/// @custom:security-contact security@taiko.xyz
library Lib1559Math {
    using LibMath for uint256;

    error EIP1559_INVALID_PARAMS();

    function calc1559BaseFee(
        uint32 _gasTargetPerL1Block,
        uint8 _adjustmentQuotient,
        uint64 _gasExcess,
        uint64 _gasIssuance,
        uint32 _parentGasUsed
    )
        internal
        pure
        returns (uint256 basefee_, uint64 gasExcess_)
    {
        // We always add the gas used by parent block to the gas excess
        // value as this has already happened
        uint256 excess = uint256(_gasExcess) + _parentGasUsed;
        excess = excess > _gasIssuance ? excess - _gasIssuance : 1;
        gasExcess_ = uint64(excess.min(type(uint64).max));

        // The base fee per gas used by this block is the spot price at the
        // bonding curve, regardless the actual amount of gas used by this
        // block, however, this block's gas used will affect the next
        // block's base fee.
        basefee_ = basefee(gasExcess_, uint256(_adjustmentQuotient) * _gasTargetPerL1Block);

        // Always make sure basefee is nonzero, this is required by the node.
        if (basefee_ == 0) basefee_ = 1;
    }

    /// @dev eth_qty(excess_gas_issued) / (TARGET * ADJUSTMENT_QUOTIENT)
    /// @param _gasExcess The gas excess value
    /// @param _adjustmentFactor The product of gasTarget and adjustmentQuotient
    function basefee(
        uint256 _gasExcess,
        uint256 _adjustmentFactor
    )
        internal
        pure
        returns (uint256)
    {
        if (_adjustmentFactor == 0) {
            revert EIP1559_INVALID_PARAMS();
        }
        return _ethQty(_gasExcess, _adjustmentFactor) / LibFixedPointMath.SCALING_FACTOR;
    }

    /// @dev exp(gas_qty / TARGET / ADJUSTMENT_QUOTIENT)
    function _ethQty(
        uint256 _gasExcess,
        uint256 _adjustmentFactor
    )
        private
        pure
        returns (uint256)
    {
        uint256 input = _gasExcess * LibFixedPointMath.SCALING_FACTOR / _adjustmentFactor;
        if (input > LibFixedPointMath.MAX_EXP_INPUT) {
            input = LibFixedPointMath.MAX_EXP_INPUT;
        }
        return uint256(LibFixedPointMath.exp(int256(input)));
    }
}
