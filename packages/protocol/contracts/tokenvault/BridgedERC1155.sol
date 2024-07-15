// SPDX-License-Identifier: MIT
pragma solidity 0.8.24;

import "@openzeppelin/contracts-upgradeable/token/ERC1155/ERC1155Upgradeable.sol";
import "../common/EssentialContract.sol";
import "../common/LibStrings.sol";
import "./IBridgedERC1155.sol";
import "./LibBridgedToken.sol";

/// @title BridgedERC1155
/// @notice Contract for bridging ERC1155 tokens across different chains.
/// @custom:security-contact security@taiko.xyz
contract BridgedERC1155 is
    EssentialContract,
    IBridgedERC1155,
    IBridgedERC1155Initializable,
    ERC1155Upgradeable
{
    /// @notice Address of the source token contract.
    address public srcToken;

    /// @notice Source chain ID where the token originates.
    uint256 public srcChainId;

    /// @dev Symbol of the bridged token.
    string public symbol;

    /// @dev Name of the bridged token.
    string public name;

    uint256[46] private __gap;

    error BTOKEN_INVALID_PARAMS();

    /// @inheritdoc IBridgedERC1155Initializable
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
        // Check if provided parameters are valid.
        // The symbol and the name can be empty for ERC1155 tokens so we use some placeholder data
        // for them instead.
        LibBridgedToken.validateInputs(_srcToken, _srcChainId);
        __Essential_init(_owner, _addressManager);

        // The token URI here is not important as the client will have to read the URI from the
        // canonical contract to fetch meta data.
        __ERC1155_init(LibBridgedToken.buildURI(_srcToken, _srcChainId, ""));

        srcToken = _srcToken;
        srcChainId = _srcChainId;
        symbol = _symbol;
        name = _name;
    }

    /// @inheritdoc IBridgedERC1155
    function mintBatch(
        address _to,
        uint256[] calldata _tokenIds,
        uint256[] calldata _amounts
    )
        external
        whenNotPaused
        onlyFromNamed(LibStrings.B_ERC1155_VAULT)
        nonReentrant
    {
        _mintBatch(_to, _tokenIds, _amounts, "");
    }

    /// @inheritdoc IBridgedERC1155
    function burn(
        uint256 _id,
        uint256 _amount
    )
        external
        whenNotPaused
        onlyFromNamed(LibStrings.B_ERC1155_VAULT)
        nonReentrant
    {
        _burn(msg.sender, _id, _amount);
    }

    /// @inheritdoc IBridgedERC1155
    function canonical() external view returns (address, uint256) {
        return (srcToken, srcChainId);
    }

    function supportsInterface(bytes4 _interfaceId) public view override returns (bool) {
        return _interfaceId == type(IBridgedERC1155).interfaceId
            || _interfaceId == type(IBridgedERC1155Initializable).interfaceId
            || super.supportsInterface(_interfaceId);
    }

    function _beforeTokenTransfer(
        address _operator,
        address _from,
        address _to,
        uint256[] memory _ids,
        uint256[] memory _amounts,
        bytes memory _data
    )
        internal
        override
        whenNotPaused
    {
        LibBridgedToken.checkToAddress(_to);
        super._beforeTokenTransfer(_operator, _from, _to, _ids, _amounts, _data);
    }
}
