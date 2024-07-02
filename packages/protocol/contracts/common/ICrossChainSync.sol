// SPDX-License-Identifier: MIT
//  _____     _ _         _         _
// |_   _|_ _(_) |_____  | |   __ _| |__ ___
//   | |/ _` | | / / _ \ | |__/ _` | '_ (_-<
//   |_|\__,_|_|_\_\___/ |____\__,_|_.__/__/

pragma solidity ^0.8.20;

/// @title ICrossChainSync
/// @dev This interface is implemented by both the TaikoL1 and TaikoL2
/// contracts.
/// It outlines the essential methods required for synchronizing and accessing
/// block hashes across chains. The core idea is to ensure that data between
/// both chains remain consistent and can be cross-referenced with integrity.
interface ICrossChainSync {
    struct Snippet {
        uint64 remoteBlockId;
        uint64 syncedInBlock;
        bytes32 blockHash;
        bytes32 signalRoot;
    }

    /// @dev Emitted when a block has been synced across chains.
    /// @param syncedInBlock The ID of this chain's block where the sync
    /// happened.
    /// @param blockId The ID of the remote block whose block hash and
    /// signal root are synced.
    /// @param blockHash The hash of the synced block.
    /// @param signalRoot The root hash representing cross-chain signals.
    event CrossChainSynced(
        uint64 indexed syncedInBlock, uint64 indexed blockId, bytes32 blockHash, bytes32 signalRoot
    );

    /// @notice Fetches the hash of a block from the opposite chain.
    /// @param blockId The target block id. Specifying 0 retrieves the hash
    /// of the latest block.
    /// @return snippet The block hash and signal root synced.
    function getSyncedSnippet(uint64 blockId) external view returns (Snippet memory snippet);
}
