// SPDX-License-Identifier: MIT
//  _____     _ _         _         _
// |_   _|_ _(_) |_____  | |   __ _| |__ ___
//   | |/ _` | | / / _ \ | |__/ _` | '_ (_-<
//   |_|\__,_|_|_\_\___/ |____\__,_|_.__/__/

pragma solidity ^0.8.20;

import "@openzeppelin/contracts/token/ERC20/ERC20.sol";

// An ERC20 token for testing the Taiko Bridge on testnets.
// This token has 50% of failure on transfers so we can
// test the bridge's error handling.
contract MayFailFreeMintERC20 is ERC20 {
    mapping(address minter => bool hasMinted) public minters;

    error HasMinted();
    error Failed();

    constructor(string memory name, string memory symbol) ERC20(name, symbol) { }

    function mint(address to) public {
        if (minters[msg.sender]) {
            revert HasMinted();
        }

        minters[msg.sender] = true;
        _mint(to, 50 * (10 ** decimals()));
    }

    function transfer(address to, uint256 amount) public override returns (bool) {
        _mayFail();
        return ERC20.transfer(to, amount);
    }

    function transferFrom(
        address from,
        address to,
        uint256 amount
    )
        public
        override
        returns (bool)
    {
        _mayFail();
        return ERC20.transferFrom(from, to, amount);
    }

    // Have a 50% change of failure.
    function _mayFail() private view {
        if (block.number % 2 == 0) {
            revert Failed();
        }
    }
}
