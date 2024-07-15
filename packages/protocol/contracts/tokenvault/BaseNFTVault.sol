// SPDX-License-Identifier: MIT
pragma solidity 0.8.24;

import "./BaseVault.sol";

/// @title BaseNFTVault
/// @notice Abstract contract for bridging NFTs across different chains.
/// @custom:security-contact security@taiko.xyz
abstract contract BaseNFTVault is BaseVault {
    // Struct representing the canonical NFT on another chain.
    struct CanonicalNFT {
        // Chain ID of the NFT.
        uint64 chainId;
        // Address of the NFT contract.
        address addr;
        // Symbol of the NFT.
        string symbol;
        // Name of the NFT.
        string name;
    }

    /// @devStruct representing the details of a bridged token transfer operation.
    /// 5 slots
    struct BridgeTransferOp {
        // Destination chain ID.
        uint64 destChainId;
        // The owner of the bridge message on the destination chain.
        address destOwner;
        // Recipient address.
        address to;
        // Processing fee for the relayer.
        uint64 fee;
        // Address of the token.
        address token;
        // Gas limit for the operation.
        uint32 gasLimit;
        // Token Id array
        uint256[] tokenIds;
        // Respective amounts per given token Ids.
        uint256[] amounts;
    }

    /// @notice Mapping to store bridged NFTs and their canonical counterparts.
    mapping(address btoken => CanonicalNFT canonical) public bridgedToCanonical;

    /// @notice Mapping to store canonical NFTs and their bridged counterparts.
    mapping(uint256 chainId => mapping(address ctoken => address btoken)) public canonicalToBridged;

    uint256[48] private __gap;

    /// @notice Emitted when a new bridged token is deployed.
    /// @param chainId The chain ID of the bridged token.
    /// @param ctoken The address of the canonical token.
    /// @param btoken The address of the bridged token.
    /// @param ctokenSymbol The symbol of the canonical token.
    /// @param ctokenName The name of the canonical token.
    event BridgedTokenDeployed(
        uint64 indexed chainId,
        address indexed ctoken,
        address indexed btoken,
        string ctokenSymbol,
        string ctokenName
    );

    /// @notice Emitted when a token is sent to another chain.
    /// @param msgHash The hash of the message.
    /// @param from The sender of the message.
    /// @param to The recipient of the message.
    /// @param destChainId The destination chain ID.
    /// @param ctoken The address of the canonical token.
    /// @param token The address of the bridged token.
    /// @param tokenIds The IDs of the tokens.
    /// @param amounts The amounts of the tokens.
    event TokenSent(
        bytes32 indexed msgHash,
        address indexed from,
        address indexed to,
        uint64 destChainId,
        address ctoken,
        address token,
        uint256[] tokenIds,
        uint256[] amounts
    );

    /// @notice Emitted when a token is released on the current chain.
    /// @param msgHash The hash of the message.
    /// @param from The sender of the message.
    /// @param ctoken The address of the canonical token.
    /// @param token The address of the bridged token.
    /// @param tokenIds The IDs of the tokens.
    /// @param amounts The amounts of the tokens.
    event TokenReleased(
        bytes32 indexed msgHash,
        address indexed from,
        address ctoken,
        address token,
        uint256[] tokenIds,
        uint256[] amounts
    );

    /// @notice Emitted when a token is received from another chain.
    /// @param msgHash The hash of the message.
    /// @param from The sender of the message.
    /// @param to The recipient of the message.
    /// @param srcChainId The source chain ID.
    /// @param ctoken The address of the canonical token.
    /// @param token The address of the bridged token.
    /// @param tokenIds The IDs of the tokens.
    /// @param amounts The amounts of the tokens.
    event TokenReceived(
        bytes32 indexed msgHash,
        address indexed from,
        address indexed to,
        uint64 srcChainId,
        address ctoken,
        address token,
        uint256[] tokenIds,
        uint256[] amounts
    );

    error VAULT_INVALID_TOKEN();
    error VAULT_INVALID_AMOUNT();
    error VAULT_INTERFACE_NOT_SUPPORTED();
    error VAULT_TOKEN_ARRAY_MISMATCH();

    modifier withValidOperation(BridgeTransferOp memory _op) {
        if (_op.tokenIds.length != _op.amounts.length) {
            revert VAULT_TOKEN_ARRAY_MISMATCH();
        }

        if (_op.token == address(0)) revert VAULT_INVALID_TOKEN();
        _;
    }
}
