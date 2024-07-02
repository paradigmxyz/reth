// SPDX-License-Identifier: MIT
//  _____     _ _         _         _
// |_   _|_ _(_) |_____  | |   __ _| |__ ___
//   | |/ _` | | / / _ \ | |__/ _` | '_ (_-<
//   |_|\__,_|_|_\_\___/ |____\__,_|_.__/__/

pragma solidity ^0.8.20;

import "lib/openzeppelin-contracts-upgradeable/contracts/token/ERC20/ERC20Upgradeable.sol";
import "../common/EssentialContract.sol";
import "./IBridgedERC20.sol";

abstract contract BridgedERC20Base is EssentialContract, IBridgedERC20 {
    address public migratingAddress; // slot 1
    bool public migratingInbound;
    uint256[49] private __gap;

    event MigrationStatusChanged(address addr, bool inbound);

    event MigratedTo(address indexed fromToken, address indexed account, uint256 amount);
    event MigratedFrom(address indexed toToken, address indexed account, uint256 amount);

    error BB_PERMISSION_DENIED();
    error BB_INVALID_PARAMS();
    error BB_MINT_DISALLOWED();

    function changeMigrationStatus(
        address addr,
        bool inbound
    )
        external
        whenNotPaused
        onlyFromOwnerOrNamed("erc20_vault")
    {
        if (addr == migratingAddress && inbound == migratingInbound) {
            revert BB_INVALID_PARAMS();
        }

        migratingAddress = addr;
        migratingInbound = inbound;
        emit MigrationStatusChanged(addr, inbound);
    }

    function mint(address account, uint256 amount) public nonReentrant whenNotPaused {
        // mint is disabled while migrating outbound.
        if (migratingAddress != address(0) && !migratingInbound) revert BB_MINT_DISALLOWED();

        if (msg.sender == migratingAddress) {
            // Inbound migration
            emit MigratedTo(migratingAddress, account, amount);
        } else if (msg.sender != resolve("erc20_vault", true)) {
            // Bridging from vault
            revert BB_PERMISSION_DENIED();
        }

        _mintToken(account, amount);
    }

    function burn(address account, uint256 amount) public nonReentrant whenNotPaused {
        if (migratingAddress != address(0) && !migratingInbound) {
            // Outbond migration
            emit MigratedTo(migratingAddress, account, amount);
            // Ask the new bridged token to mint token for the user.
            IBridgedERC20(migratingAddress).mint(account, amount);
        } else if (msg.sender != resolve("erc20_vault", true)) {
            // Bridging to vault
            revert RESOLVER_DENIED();
        }

        _burnToken(account, amount);
    }

    function owner() public view override(IBridgedERC20, OwnableUpgradeable) returns (address) {
        return super.owner();
    }

    function _mintToken(address account, uint256 amount) internal virtual;
    function _burnToken(address from, uint256 amount) internal virtual;
}
