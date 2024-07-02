// SPDX-License-Identifier: MIT
//  _____     _ _         _         _
// |_   _|_ _(_) |_____  | |   __ _| |__ ___
//   | |/ _` | | / / _ \ | |__/ _` | '_ (_-<
//   |_|\__,_|_|_\_\___/ |____\__,_|_.__/__/

pragma solidity ^0.8.20;

import "@openzeppelin/contracts/proxy/ERC1967/ERC1967Proxy.sol";
import "@openzeppelin/contracts-upgradeable/access/OwnableUpgradeable.sol";

/// @title LibDeploy
/// @dev Provides utilities for deploying contracts
library LibDeploy {
    error NULL_IMPL_ADDR();

    function deployERC1967Proxy(
        address impl,
        address owner,
        bytes memory data
    )
        internal
        returns (address proxy)
    {
        if (impl == address(0)) revert NULL_IMPL_ADDR();
        proxy = address(new ERC1967Proxy(impl, data));

        if (owner != address(0) && owner != OwnableUpgradeable(proxy).owner()) {
            OwnableUpgradeable(proxy).transferOwnership(owner);
        }
    }
}
