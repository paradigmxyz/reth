// SPDX-License-Identifier: MIT
//  _____     _ _         _         _
// |_   _|_ _(_) |_____  | |   __ _| |__ ___
//   | |/ _` | | / / _ \ | |__/ _` | '_ (_-<
//   |_|\__,_|_|_\_\___/ |____\__,_|_.__/__/

pragma solidity ^0.8.20;

import "lib/openzeppelin-contracts-upgradeable/contracts/token/ERC721/ERC721Upgradeable.sol";
import "lib/openzeppelin-contracts/contracts/utils/Strings.sol";
import "../common/EssentialContract.sol";
import "./LibBridgedToken.sol";

/// @title BridgedERC721
/// @notice Contract for bridging ERC721 tokens across different chains.
contract BridgedERC721 is EssentialContract, ERC721Upgradeable {
    address public srcToken; // Address of the source token contract.
    uint256 public srcChainId; // Source chain ID where the token originates.

    uint256[48] private __gap;

    error BTOKEN_CANNOT_RECEIVE();
    error BTOKEN_INVALID_PARAMS();
    error BTOKEN_INVALID_BURN();

    /// @dev Initializer function to be called after deployment.
    /// @param _addressManager The address of the address manager.
    /// @param _srcToken Address of the source token.
    /// @param _srcChainId Source chain ID.
    /// @param _symbol Symbol of the bridged token.
    /// @param _name Name of the bridged token.
    function init(
        address _addressManager,
        address _srcToken,
        uint256 _srcChainId,
        string memory _symbol,
        string memory _name
    )
        external
        initializer
    {
        if (
            _srcToken == address(0) || _srcChainId == 0 || _srcChainId == block.chainid
                || bytes(_symbol).length == 0 || bytes(_name).length == 0
        ) {
            revert BTOKEN_INVALID_PARAMS();
        }
        __Essential_init(_addressManager);
        __ERC721_init(_name, _symbol);
        srcToken = _srcToken;
        srcChainId = _srcChainId;
    }

    /// @dev Mints tokens.
    /// @param account Address to receive the minted token.
    /// @param tokenId ID of the token to mint.
    function mint(
        address account,
        uint256 tokenId
    )
        public
        nonReentrant
        whenNotPaused
        onlyFromNamed("erc721_vault")
    {
        _mint(account, tokenId);
    }

    /// @dev Burns tokens.
    /// @param account Address from which the token is burned.
    /// @param tokenId ID of the token to burn.
    function burn(
        address account,
        uint256 tokenId
    )
        public
        nonReentrant
        whenNotPaused
        onlyFromNamed("erc721_vault")
    {
        // Check if the caller is the owner of the token.
        if (ownerOf(tokenId) != account) {
            revert BTOKEN_INVALID_BURN();
        }
        _burn(tokenId);
    }

    /// @dev Safely transfers tokens from one address to another.
    /// @param from Address from which the token is transferred.
    /// @param to Address to which the token is transferred.
    /// @param tokenId ID of the token to transfer.
    function transferFrom(
        address from,
        address to,
        uint256 tokenId
    )
        public
        override(ERC721Upgradeable)
        nonReentrant
        whenNotPaused
    {
        if (to == address(this)) {
            revert BTOKEN_CANNOT_RECEIVE();
        }
        return ERC721Upgradeable.transferFrom(from, to, tokenId);
    }

    /// @notice Gets the name of the token.
    /// @return The name.
    function name() public view override(ERC721Upgradeable) returns (string memory) {
        return LibBridgedToken.buildName(super.name(), srcChainId);
    }

    /// @notice Gets the symbol of the bridged token.
    /// @return The symbol.
    function symbol() public view override(ERC721Upgradeable) returns (string memory) {
        return LibBridgedToken.buildSymbol(super.symbol());
    }

    /// @notice Gets the source token and source chain ID being bridged.
    /// @return Source token address and source chain ID.
    function source() public view returns (address, uint256) {
        return (srcToken, srcChainId);
    }

    /// @notice Returns an empty token URI.
    function tokenURI(uint256) public pure virtual override returns (string memory) {
        return "";
    }
}
