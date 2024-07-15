// SPDX-License-Identifier: MIT
pragma solidity 0.8.24;

import "../TaikoTest.sol";
import "../../contracts/L2/LibL2Config.sol";

contract TestLib1559Math is TaikoTest {
    using LibMath for uint256;

    // function test_eip1559_math() external pure {
    //     LibL2Config.Config memory config = LibL2Config.get();
    //     uint256 adjustmentFactor = config.gasTargetPerL1Block * config.basefeeAdjustmentQuotient;

    //     uint256 baseFee;
    //     uint256 i;
    //     uint256 target = 0.01 gwei;

    //     for (uint256 k; k < 5; ++k) {
    //         for (; baseFee < target; ++i) {
    //             baseFee = Lib1559Math.basefee(config.gasTargetPerL1Block * i, adjustmentFactor);
    //         }
    //         console2.log("base fee:", baseFee);
    //         console2.log("    gasExcess:", config.gasTargetPerL1Block * i);
    //         console2.log("    i:", i);
    //         target *= 10;
    //     }
    // }

    // function test_eip1559_math_max() external pure {
    //     LibL2Config.Config memory config = LibL2Config.get();
    //     uint256 adjustmentFactor = config.gasTargetPerL1Block * config.basefeeAdjustmentQuotient;

    //     uint256 gasExcess = type(uint64).max;
    //     uint256 baseFee = Lib1559Math.basefee(gasExcess, adjustmentFactor);

    //     console2.log("base fee (gwei):", baseFee / 1 gwei);
    //     console2.log("    gasExcess:", gasExcess);
    // }
}
