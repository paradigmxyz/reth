// SPDX-License-Identifier: MIT
//  _____     _ _         _         _
// |_   _|_ _(_) |_____  | |   __ _| |__ ___
//   | |/ _` | | / / _ \ | |__/ _` | '_ (_-<
//   |_|\__,_|_|_\_\___/ |____\__,_|_.__/__/

pragma solidity ^0.8.20;

import "../common/EssentialContract.sol";

/// @title AuthorizableContract
abstract contract AuthorizableContract is EssentialContract {
    mapping(address => bytes32 label) public authorizedAddresses;
    uint256[49] private __gap;

    event Authorized(address indexed addr, bytes32 oldLabel, bytes32 newLabel);

    error INVALID_ADDRESS();
    error INVALID_LABEL();

    function authorize(address addr, bytes32 label) external onlyOwner {
        if (addr == address(0)) revert INVALID_ADDRESS();

        bytes32 oldLabel = authorizedAddresses[addr];
        if (oldLabel == label) revert INVALID_LABEL();
        authorizedAddresses[addr] = label;

        emit Authorized(addr, oldLabel, label);
    }

    function isAuthorizedAs(address addr, bytes32 label) public view returns (bool) {
        return label != 0 && authorizedAddresses[addr] == label;
    }
}
