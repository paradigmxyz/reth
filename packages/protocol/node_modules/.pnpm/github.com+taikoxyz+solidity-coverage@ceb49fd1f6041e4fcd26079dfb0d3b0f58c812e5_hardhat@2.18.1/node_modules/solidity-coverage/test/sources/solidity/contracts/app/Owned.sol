pragma solidity ^0.7.0;

contract Owned {
    constructor() public { owner = msg.sender; }
    address owner;
}
