// SPDX-License-Identifier: MIT
// The MIT License (MIT)
//
// Copyright (c) 2021 Remco Bloemen
// Copyright (c) 2022-2023 Taiko Labs
//
// Permission is hereby granted, free of charge, to any person obtaining
// a copy of this software and associated documentation files (the
// "Software"), to deal in the Software without restriction, including
// without limitation the rights to use, copy, modify, merge, publish,
// distribute, sublicense, and/or sell copies of the Software, and to
// permit persons to whom the Software is furnished to do so, subject to
// the following conditions:
//
// The above copyright notice and this permission notice shall be
// included in all copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
// EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
// MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.
// IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
// CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT,
// TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE
// SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

pragma solidity ^0.8.20;

/// @title LibUint512Math
library LibUint512Math {
    /// @dev Multiplies two uint256 numbers to return a 512-bit result.
    /// Taken from: https://xn--2-umb.com/17/full-mul/index.html
    /// @param a The first uint256 operand.
    /// @param b The second uint256 operand.
    /// @return r0 The lower 256 bits of the result.
    /// @return r1 The higher 256 bits of the result.
    function mul(uint256 a, uint256 b) internal pure returns (uint256 r0, uint256 r1) {
        assembly {
            // Calculate modulo of the multiplication by the largest 256-bit
            // number.
            let mm := mulmod(a, b, not(0))
            // Standard 256-bit multiplication.
            r0 := mul(a, b)
            // Adjust for overflow, detect if there was a carry.
            r1 := sub(sub(mm, r0), lt(mm, r0))
        }
    }

    /// @dev Adds two 512-bit numbers represented by two pairs of uint256.
    /// Taken from:
    /// https://xn--2-umb.com/17/512-bit-division/#add-subtract-two-512-bit-numbers
    /// @param a0 The lower 256 bits of the first number.
    /// @param a1 The higher 256 bits of the first number.
    /// @param b0 The lower 256 bits of the second number.
    /// @param b1 The higher 256 bits of the second number.
    /// @return r0 The lower 256 bits of the result.
    /// @return r1 The higher 256 bits of the result.
    function add(
        uint256 a0,
        uint256 a1,
        uint256 b0,
        uint256 b1
    )
        internal
        pure
        returns (uint256 r0, uint256 r1)
    {
        assembly {
            // Standard 256-bit addition for lower bits.
            r0 := add(a0, b0)
            // Add the upper bits and account for carry from the lower bits.
            r1 := add(add(a1, b1), lt(r0, a0))
        }
    }
}
