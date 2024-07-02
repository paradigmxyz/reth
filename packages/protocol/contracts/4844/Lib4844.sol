// SPDX-License-Identifier: MIT
//  _____     _ _         _         _
// |_   _|_ _(_) |_____  | |   __ _| |__ ___
//   | |/ _` | | / / _ \ | |__/ _` | '_ (_-<
//   |_|\__,_|_|_\_\___/ |____\__,_|_.__/__/

pragma solidity ^0.8.20;

/// @title Lib4844
/// @notice A library for handling EIP-4844 blobs
/// `solc contracts/libs/Lib4844.sol --ir > contracts/libs/Lib4844.yul`
library Lib4844 {
    address public constant POINT_EVALUATION_PRECOMPILE_ADDRESS = address(0x0A);
    uint32 public constant FIELD_ELEMENTS_PERBLOB = 4096;
    uint256 public constant BLS_MODULUS =
        52_435_875_175_126_190_479_447_740_508_185_965_837_690_552_500_527_637_822_603_658_699_938_581_184_513;

    error EVAL_FAILED();
    error POINT_X_TOO_LARGE();
    error POINT_Y_TOO_LARGE();

    /// @notice Evaluates the 4844 point using the precompile.
    /// @param blobHash The versioned hash
    /// @param x The evaluation point
    /// @param y The expected output
    /// @param commitment The input kzg point
    /// @param pointProof The quotient kzg
    function evaluatePoint(
        bytes32 blobHash,
        uint256 x,
        uint256 y,
        bytes1[48] memory commitment,
        bytes1[48] memory pointProof
    )
        internal
        view
    {
        if (x >= BLS_MODULUS) revert POINT_X_TOO_LARGE();
        if (y >= BLS_MODULUS) revert POINT_Y_TOO_LARGE();

        (bool ok,) = POINT_EVALUATION_PRECOMPILE_ADDRESS.staticcall(
            abi.encodePacked(blobHash, x, y, commitment, pointProof)
        );
        if (!ok) revert EVAL_FAILED();
    }
}
