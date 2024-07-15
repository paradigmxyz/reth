// SPDX-License-Identifier: MIT
//  _____     _ _         _         _
// |_   _|_ _(_) |_____  | |   __ _| |__ ___
//   | |/ _` | | / / _ \ | |__/ _` | '_ (_-<
//   |_|\__,_|_|_\_\___/ |____\__,_|_.__/__/

pragma solidity ^0.8.20;

/// @title TaikoData
/// @notice This library defines various data structures used in the Taiko
/// protocol.
library TaikoData {
    /// @dev Struct holding Taiko configuration parameters. See {TaikoConfig}.
    struct Config {
        // The chain ID of the network where Taiko contracts are deployed.
        uint64 chainId;
        // The maximum gas limit allowed for a block.
        uint32 blockMaxGasLimit;
        // The maximum allowed bytes for the proposed transaction list calldata.
        uint24 blockMaxTxListBytes;
    }

    /// @dev Struct containing data only required for proving a block
    struct BlockMetadata {
        bytes32 blockHash;
        bytes32 parentBlockHash;
        bytes32 parentMetaHash;
        bytes32 l1Hash;
        uint256 difficulty;
        bytes32 blobHash;
        bytes32 extraData;
        address coinbase;
        uint64 l2BlockNumber;
        uint32 gasLimit;
        uint32 l1StateBlockNumber;
        uint64 timestamp;
        uint24 txListByteOffset;
        uint24 txListByteSize;
        // todo: Do we need this below ?
        // bytes32 blobId OR blobHash; ? as per in current taiko-mono's preconfirmation branch ?
        bool blobUsed;
    }

    /// @dev Struct representing transition to be proven.
    struct Transition {
        bytes32 parentBlockHash;
        bytes32 blockHash;
    }

    /// @dev Struct representing state transition data.
    struct TransitionState {
        bytes32 blockHash; //Might be removed..
        uint64 timestamp;
        address prover;
        uint64 verifiableAfter;
        bool isProven;
    }

    /// @dev Struct containing data required for verifying a block.
    struct Block {
        bytes32 blockHash;
        bytes32 metaHash;
        uint64 blockId;
        uint64 timestamp;
        uint32 l1StateBlockNumber;
    }

    /// @dev Struct holding the state variables for the {TaikoL1} contract.
    struct State {
        mapping(uint256 blockId => Block) blocks;
        mapping(uint256 blockId => mapping(bytes32 parentBlockHash => TransitionState)) transitions;
        uint64 genesisHeight;
        uint64 genesisTimestamp;
        uint64 numBlocks;
        uint64 lastVerifiedBlockId;
        bool provingPaused;
        uint64 lastUnpausedAt;
        uint256[143] __gap;
    }
}
