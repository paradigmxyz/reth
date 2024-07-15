// SPDX-License-Identifier: MIT
// Taken from the contract below, expWad() function tailored to Taiko's need
// https://github.com/transmissions11/solmate/blob/v7/src/utils/FixedPointMathLib.sol
pragma solidity 0.8.24;

library LibFixedPointMath {
    uint128 public constant MAX_EXP_INPUT = 135_305_999_368_893_231_588;
    uint256 public constant SCALING_FACTOR = 1e18; // For fixed point representation factor

    error Overflow();

    // Computes e^x in 1e18 fixed point.
    function exp(int256 x) internal pure returns (int256 r) {
        unchecked {
            // Input x is in fixed point format, with scale factor 1/1e18.

            // When the result is < 0.5 we return zero. This happens when
            // x <= floor(log(0.5e18) * 1e18) ~ -42e18
            if (x <= -42_139_678_854_452_767_551) {
                return 0;
            }

            // When the result is > (2**255 - 1) / 1e18 we can not represent it
            // as an int256. This happens when x >= floor(log((2**255 -1) /
            // 1e18) * 1e18) ~ 135.
            if (x >= 135_305_999_368_893_231_589) revert Overflow();

            // x is now in the range (-42, 136) * 1e18. Convert to (-42, 136) *
            // 2**96
            // for more intermediate precision and a binary basis. This base
            // conversion
            // is a multiplication by 1e18 / 2**96 = 5**18 / 2**78.
            x = (x << 78) / 5 ** 18;

            // Reduce range of x to (-½ ln 2, ½ ln 2) * 2**96 by factoring out
            // powers of two
            // such that exp(x) = exp(x') * 2**k, where k is an integer.
            // Solving this gives k = round(x / log(2)) and x' = x - k * log(2).
            int256 k = ((x << 96) / 54_916_777_467_707_473_351_141_471_128 + 2 ** 95) >> 96;
            x = x - k * 54_916_777_467_707_473_351_141_471_128;
            // k is in the range [-61, 195].

            // Evaluate using a (6, 7)-term rational approximation.
            // p is made monic, we'll multiply by a scale factor later.
            int256 y = x + 1_346_386_616_545_796_478_920_950_773_328;
            y = ((y * x) >> 96) + 57_155_421_227_552_351_082_224_309_758_442;
            int256 p = y + x - 94_201_549_194_550_492_254_356_042_504_812;
            p = ((p * y) >> 96) + 28_719_021_644_029_726_153_956_944_680_412_240;
            p = p * x + (4_385_272_521_454_847_904_659_076_985_693_276 << 96);

            // We leave p in 2**192 basis so we don't need to scale it back up
            // for the division.
            int256 q = x - 2_855_989_394_907_223_263_936_484_059_900;
            q = ((q * x) >> 96) + 50_020_603_652_535_783_019_961_831_881_945;
            q = ((q * x) >> 96) - 533_845_033_583_426_703_283_633_433_725_380;
            q = ((q * x) >> 96) + 3_604_857_256_930_695_427_073_651_918_091_429;
            q = ((q * x) >> 96) - 14_423_608_567_350_463_180_887_372_962_807_573;
            q = ((q * x) >> 96) + 26_449_188_498_355_588_339_934_803_723_976_023;
            assembly {
                // Div in assembly because solidity adds a zero check despite
                // the `unchecked`.
                // The q polynomial is known not to have zeros in the domain.
                // (All roots are complex)
                // No scaling required because p is already 2**96 too large.
                r := sdiv(p, q)
            }
            // r should be in the range (0.09, 0.25) * 2**96.

            // We now need to multiply r by
            //  * the scale factor s = ~6.031367120...,
            //  * the 2**k factor from the range reduction, and
            //  * the 1e18 / 2**96 factor for base conversion.
            // We do all of this at once, with an intermediate result in 2**213
            // basis
            // so the final right shift is always by a positive amount.
            r = int256(
                (uint256(r) * 3_822_833_074_963_236_453_042_738_258_902_158_003_155_416_615_667)
                    >> uint256(195 - k)
            );
        }
    }
}
