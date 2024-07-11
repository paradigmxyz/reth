// SPDX-License-Identifier: MIT
pragma solidity 0.8.24;

/// @title IBridgedERC721
/// @notice Contract for bridging ERC721 tokens across different chains.
/// @custom:security-contact security@taiko.xyz
interface IBridgedERC721 {
    /// @dev Mints tokens.
    /// @param _account Address to receive the minted token.
    /// @param _tokenId ID of the token to mint.
    function mint(address _account, uint256 _tokenId) external;

    /// @dev Burns tokens.
    /// @param _tokenId ID of the token to burn.
    function burn(uint256 _tokenId) external;

    /// @notice Gets the canonical token's address and chain ID.
    /// @return The canonical token's address.
    /// @return The canonical token's chain ID.
    function canonical() external view returns (address, uint256);
}

/// @title IBridgedERC721Initializable
/// @custom:security-contact security@taiko.xyz
interface IBridgedERC721Initializable {
    /// @notice Initializes the contract.
    /// @param _owner The owner of this contract. msg.sender will be used if this value is zero.
    /// @param _addressManager The address of the {AddressManager} contract.
    /// @param _srcToken Address of the source token.
    /// @param _srcChainId Source chain ID.
    /// @param _symbol Symbol of the bridged token.
    /// @param _name Name of the bridged token.
    function init(
        address _owner,
        address _addressManager,
        address _srcToken,
        uint256 _srcChainId,
        string calldata _symbol,
        string calldata _name
    )
        external;
}
