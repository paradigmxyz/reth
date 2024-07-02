// SPDX-License-Identifier: MIT
//  _____     _ _         _         _
// |_   _|_ _(_) |_____  | |   __ _| |__ ___
//   | |/ _` | | / / _ \ | |__/ _` | '_ (_-<
//   |_|\__,_|_|_\_\___/ |____\__,_|_.__/__/

pragma solidity ^0.8.20;

import "../BridgedERC20Base.sol";

interface IUSDC {
    function burn(uint256 amount) external;
    function mint(address to, uint256 amount) external;
    function transferFrom(address from, address to, uint256 amount) external;
}

/// @title USDCAdaptor
contract USDCAdaptor is BridgedERC20Base {
    IUSDC public usdc; // slot 1
    uint256[49] private __gap;

    function init(address _adressManager, IUSDC _usdc) external initializer {
        __Essential_init(_adressManager);
        usdc = _usdc;
    }

    function _mintToken(address account, uint256 amount) internal override {
        usdc.mint(account, amount);
    }

    function _burnToken(address from, uint256 amount) internal override {
        usdc.transferFrom(from, address(this), amount);
        usdc.burn(amount);
    }
}
