// SPDX-License-Identifier: MIT
pragma solidity 0.8.24;

import "@openzeppelin/contracts/utils/Address.sol";
import "@openzeppelin/contracts/utils/cryptography/ECDSA.sol";
import "@openzeppelin/contracts/utils/introspection/IERC165.sol";
import "@openzeppelin/contracts/interfaces/IERC1271.sol";

/// @title LibAddress
/// @dev Provides utilities for address-related operations.
/// @custom:security-contact security@taiko.xyz
library LibAddress {
    bytes4 private constant EIP1271_MAGICVALUE = 0x1626ba7e;

    error ETH_TRANSFER_FAILED();

    /// @dev Sends Ether to the specified address. This method will not revert even if sending ether
    /// fails.
    /// This function is inspired by
    /// https://github.com/nomad-xyz/ExcessivelySafeCall/blob/main/src/ExcessivelySafeCall.sol
    /// @param _to The recipient address.
    /// @param _amount The amount of Ether to send in wei.
    /// @param _gasLimit The max amount gas to pay for this transaction.
    /// @return success_ true if the call is successful, false otherwise.
    function sendEther(
        address _to,
        uint256 _amount,
        uint256 _gasLimit,
        bytes memory _calldata
    )
        internal
        returns (bool success_)
    {
        // Check for zero-address transactions
        if (_to == address(0)) revert ETH_TRANSFER_FAILED();
        // dispatch message to recipient
        // by assembly calling "handle" function
        // we call via assembly to avoid memcopying a very large returndata
        // returned by a malicious contract
        assembly {
            success_ :=
                call(
                    _gasLimit, // gas
                    _to, // recipient
                    _amount, // ether value
                    add(_calldata, 0x20), // inloc
                    mload(_calldata), // inlen
                    0, // outloc
                    0 // outlen
                )
        }
    }

    /// @dev Sends Ether to the specified address. This method will revert if sending ether fails.
    /// @param _to The recipient address.
    /// @param _amount The amount of Ether to send in wei.
    /// @param _gasLimit The max amount gas to pay for this transaction.
    function sendEtherAndVerify(address _to, uint256 _amount, uint256 _gasLimit) internal {
        if (_amount == 0) return;
        if (!sendEther(_to, _amount, _gasLimit, "")) {
            revert ETH_TRANSFER_FAILED();
        }
    }

    /// @dev Sends Ether to the specified address. This method will revert if sending ether fails.
    /// @param _to The recipient address.
    /// @param _amount The amount of Ether to send in wei.
    function sendEtherAndVerify(address _to, uint256 _amount) internal {
        sendEtherAndVerify(_to, _amount, gasleft());
    }

    function supportsInterface(
        address _addr,
        bytes4 _interfaceId
    )
        internal
        view
        returns (bool result_)
    {
        if (!Address.isContract(_addr)) return false;

        try IERC165(_addr).supportsInterface(_interfaceId) returns (bool _result) {
            result_ = _result;
        } catch { }
    }

    function isValidSignature(
        address addr,
        bytes32 hash,
        bytes memory sig
    )
        internal
        view
        returns (bool valid)
    {
        if (Address.isContract(addr)) {
            return IERC1271(addr).isValidSignature(hash, sig) == EIP1271_MAGICVALUE;
        } else {
            return ECDSA.recover(hash, sig) == addr;
        }
    }
}
