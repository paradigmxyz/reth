// SPDX-License-Identifier: MIT
//  _____     _ _         _         _
// |_   _|_ _(_) |_____  | |   __ _| |__ ___
//   | |/ _` | | / / _ \ | |__/ _` | '_ (_-<
//   |_|\__,_|_|_\_\___/ |____\__,_|_.__/__/

pragma solidity ^0.8.20;

import
    "lib/openzeppelin-contracts-upgradeable/contracts/governance/TimelockControllerUpgradeable.sol";
import "../../common/OwnerUUPSUpgradable.sol";

contract TaikoTimelockController is OwnerUUPSUpgradable, TimelockControllerUpgradeable {
    uint256[50] private __gap;

    function init(uint256 minDelay) external initializer {
        __OwnerUUPSUpgradable_init();
        address[] memory nil = new address[](0);
        __TimelockController_init(minDelay, nil, nil, owner());
    }

    /// @dev Allows the admin to get around of the min delay.
    function getMinDelay() public view override returns (uint256) {
        return hasRole(TIMELOCK_ADMIN_ROLE, msg.sender) ? 0 : super.getMinDelay();
    }
}
