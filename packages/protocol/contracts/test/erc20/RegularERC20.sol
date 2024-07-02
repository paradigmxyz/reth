// SPDX-License-Identifier: MIT

pragma solidity ^0.8.20;

import "@openzeppelin/contracts/token/ERC20/ERC20.sol";

contract RegularERC20 is ERC20 {
    constructor(uint256 initialSupply) ERC20("RegularERC20", "RGL") {
        _mint(msg.sender, initialSupply);
    }
}
