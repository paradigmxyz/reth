// SPDX-License-Identifier: MIT
//  _____     _ _         _         _
// |_   _|_ _(_) |_____  | |   __ _| |__ ___
//   | |/ _` | | / / _ \ | |__/ _` | '_ (_-<
//   |_|\__,_|_|_\_\___/ |____\__,_|_.__/__/

pragma solidity ^0.8.20;

import "../TaikoData.sol";

/// @title IVerifier Interface
/// @notice Defines the function that handles proof verification.
interface IVerifier {
    function verifyProof(
        TaikoData.BlockMetadata calldata _block,
        TaikoData.Transition calldata transition,
        address prover,
        bytes calldata proof
    )
    external;
}
