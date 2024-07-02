pragma solidity ^0.7.0;

import "./Owned.sol";

contract Proxy is Owned {
    function isOwner() public view returns (bool) {
        if (msg.sender == owner) {
            return true;
        } else {
            return false;
        }
    }
}
