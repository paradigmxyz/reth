// SPDX-License-Identifier: MIT
pragma solidity 0.8.24;

import "@openzeppelin/contracts-upgradeable/token/ERC721/ERC721Upgradeable.sol";
import "../common/EssentialContract.sol";
import "../common/LibStrings.sol";
import "./IBridgedERC721.sol";
import "./LibBridgedToken.sol";

/// @title BridgedERC721
/// @notice Contract for bridging ERC721 tokens across different chains.
/// @custom:security-contact security@taiko.xyz
contract BridgedERC721 is
    EssentialContract,
    IBridgedERC721,
    IBridgedERC721Initializable,
    ERC721Upgradeable
{
    /// @notice Address of the source token contract.
    address public srcToken;

    /// @notice Source chain ID where the token originates.
    uint256 public srcChainId;

    uint256[48] private __gap;

    error BTOKEN_INVALID_PARAMS();
    error BTOKEN_INVALID_BURN();

    /// @inheritdoc IBridgedERC721Initializable
    function init(
        address _owner,
        address _addressManager,
        address _srcToken,
        uint256 _srcChainId,
        string calldata _symbol,
        string calldata _name
    )
        external
        initializer
    {
        // Check if provided parameters are valid
        LibBridgedToken.validateInputs(_srcToken, _srcChainId);
        __Essential_init(_owner, _addressManager);
        __ERC721_init(_name, _symbol);

        srcToken = _srcToken;
        srcChainId = _srcChainId;
    }

    /// @inheritdoc IBridgedERC721
    function mint(
        address _account,
        uint256 _tokenId
    )
        external
        whenNotPaused
        onlyFromNamed(LibStrings.B_ERC721_VAULT)
        nonReentrant
    {
        _safeMint(_account, _tokenId);
    }

    /// @inheritdoc IBridgedERC721
    function burn(uint256 _tokenId)
        external
        whenNotPaused
        onlyFromNamed(LibStrings.B_ERC721_VAULT)
        nonReentrant
    {
        // Check if the caller is the owner of the token. Somehow this is not done inside the
        // _burn() function below.
        if (ownerOf(_tokenId) != msg.sender) {
            revert BTOKEN_INVALID_BURN();
        }
        _burn(_tokenId);
    }

    /// @inheritdoc IBridgedERC721
    function canonical() external view returns (address, uint256) {
        return (srcToken, srcChainId);
    }

    /// @notice Returns the token URI.
    /// @param _tokenId The token id.
    /// @return The token URI following EIP-681.
    function tokenURI(uint256 _tokenId) public view override returns (string memory) {
        // https://github.com/crytic/slither/wiki/Detector-Documentation#abi-encodePacked-collision
        // The abi.encodePacked() call below takes multiple dynamic arguments. This is known and
        // considered acceptable in terms of risk.
        return LibBridgedToken.buildURI(srcToken, srcChainId, Strings.toString(_tokenId));
    }

    function supportsInterface(bytes4 _interfaceId) public view override returns (bool) {
        return _interfaceId == type(IBridgedERC721).interfaceId
            || _interfaceId == type(IBridgedERC721Initializable).interfaceId
            || super.supportsInterface(_interfaceId);
    }

    function _beforeTokenTransfer(
        address _from,
        address _to,
        uint256 _firstTokenId,
        uint256 _batchSize
    )
        internal
        override
        whenNotPaused
    {
        LibBridgedToken.checkToAddress(_to);
        super._beforeTokenTransfer(_from, _to, _firstTokenId, _batchSize);
    }
}
