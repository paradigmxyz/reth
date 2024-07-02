// SPDX-License-Identifier: MIT
//  _____     _ _         _         _
// |_   _|_ _(_) |_____  | |   __ _| |__ ___
//   | |/ _` | | / / _ \ | |__/ _` | '_ (_-<
//   |_|\__,_|_|_\_\___/ |____\__,_|_.__/__/

pragma solidity ^0.8.20;

/// @title IBlobHashReader
/// @dev Labeled in AddressResolver as "blob_hash_reader"
/// @dev This interface and its corresponding implementation may deprecate once
/// solidity supports the new BLOBHASH opcode natively.
interface IBlobHashReader {
    /// @notice Returns the versioned hash for the first blob in this
    /// transaction. If there is no blob found, 0x0 is returned.
    function getFirstBlobHash() external view returns (bytes32);
}
