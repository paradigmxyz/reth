// SPDX-License-Identifier: UNLICENSED
// Some of the tests are taken from:
// https://github.com/recmo/experiment-solexp/blob/main/src/test/FixedPointMathLib.t.sol
pragma solidity 0.8.24;

import "../TaikoTest.sol";

contract LibFixedPointMathTest is TaikoTest {
    function testExp1() public {
        assertEq(LibFixedPointMath.exp(-1e18), 367_879_441_171_442_321);
    }

    function testExpSmallest() public pure {
        int256 y = LibFixedPointMath.exp(-42_139_678_854_452_767_550);

        console2.log("LibFixedPointMath.exp(-42139678854452767550)=", uint256(y));
    }

    function testExpLargest() public pure {
        int256 y = LibFixedPointMath.exp(int256(uint256(LibFixedPointMath.MAX_EXP_INPUT)));
        console2.log("LibFixedPointMath.exp(135305999368893231588)=", uint256(y));
    }

    function testExpSome() public pure {
        int256 y = LibFixedPointMath.exp(5e18);
        console2.log("LibFixedPointMath.exp(5e18)=", uint256(y));
    }

    function testExpGas() public view {
        uint256 g0 = gasleft();
        LibFixedPointMath.exp(133e18);
        uint256 g1 = gasleft();
        LibFixedPointMath.exp(-23e18);
        uint256 g2 = gasleft();
        LibFixedPointMath.exp(5e18);
        uint256 g3 = gasleft();
        console2.logUint(g0 - g1);
        console2.logUint(g1 - g2);
        console2.logUint(g2 - g3);
    }

    function testExp3() public pure {
        LibFixedPointMath.exp(133e18);
        LibFixedPointMath.exp(10e18);
        LibFixedPointMath.exp(-23e18);
    }
}
