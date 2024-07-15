// SPDX-License-Identifier: MIT
pragma solidity 0.8.24;

import "@openzeppelin/contracts-upgradeable/utils/introspection/IERC165Upgradeable.sol";
import "@openzeppelin/contracts/proxy/ERC1967/ERC1967Proxy.sol";
import "../bridge/IBridge.sol";
import "../common/EssentialContract.sol";
import "../common/LibStrings.sol";
import "../libs/LibBytes.sol";

/// @title INameSymbol
/// @notice Interface for contracts that provide name() and symbol()
/// functions. These functions may not be part of the official interface but are
/// used by some contracts.
/// @custom:security-contact security@taiko.xyz
interface INameSymbol {
    function name() external view returns (string memory);
    function symbol() external view returns (string memory);
}

/// @title BaseVault
/// @notice This abstract contract provides a base implementation for vaults.
/// @custom:security-contact security@taiko.xyz
abstract contract BaseVault is
    EssentialContract,
    IRecallableSender,
    IMessageInvocable,
    IERC165Upgradeable
{
    using LibBytes for bytes;

    uint256[50] private __gap;

    error VAULT_INSUFFICIENT_FEE();
    error VAULT_INVALID_TO_ADDR();
    error VAULT_PERMISSION_DENIED();

    /// @notice Checks if the contract supports the given interface.
    /// @param _interfaceId The interface identifier.
    /// @return true if the contract supports the interface, false otherwise.
    function supportsInterface(bytes4 _interfaceId) public view virtual override returns (bool) {
        return _interfaceId == type(IRecallableSender).interfaceId
            || _interfaceId == type(IMessageInvocable).interfaceId
            || _interfaceId == type(IERC165Upgradeable).interfaceId;
    }

    /// @notice Returns the name of the vault.
    /// @return The name of the vault.
    function name() public pure virtual returns (bytes32);

    function checkProcessMessageContext()
        internal
        view
        onlyFromNamed(LibStrings.B_BRIDGE)
        returns (IBridge.Context memory ctx_)
    {
        ctx_ = IBridge(msg.sender).context();
        address selfOnSourceChain = resolve(ctx_.srcChainId, name(), false);
        if (ctx_.from != selfOnSourceChain) revert VAULT_PERMISSION_DENIED();
    }

    function checkRecallMessageContext()
        internal
        view
        onlyFromNamed(LibStrings.B_BRIDGE)
        returns (IBridge.Context memory ctx_)
    {
        ctx_ = IBridge(msg.sender).context();
        if (ctx_.from != msg.sender) revert VAULT_PERMISSION_DENIED();
    }

    function checkToAddress(address _to) internal view {
        if (_to == address(0) || _to == address(this)) revert VAULT_INVALID_TO_ADDR();
    }

    function safeSymbol(address _token) internal view returns (string memory symbol_) {
        (bool success, bytes memory data) =
            address(_token).staticcall(abi.encodeCall(INameSymbol.symbol, ()));
        return success ? data.toString() : "";
    }

    function safeName(address _token) internal view returns (string memory) {
        (bool success, bytes memory data) =
            address(_token).staticcall(abi.encodeCall(INameSymbol.name, ()));
        return success ? data.toString() : "";
    }
}
