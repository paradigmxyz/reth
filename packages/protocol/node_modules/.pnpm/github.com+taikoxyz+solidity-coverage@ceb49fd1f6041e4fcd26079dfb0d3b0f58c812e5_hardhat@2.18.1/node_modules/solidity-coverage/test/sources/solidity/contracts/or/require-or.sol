pragma solidity ^0.7.0;

contract Test {
    function a(uint x) public {
        require(x == 1 || x == 2);
    }
}
