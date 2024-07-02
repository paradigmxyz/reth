pragma solidity ^0.7.0;

contract Test {
    function a(uint x) public {
        keccak256(abi.encodePacked(x));
        keccak256(abi.encodePacked(uint256(0)));
    }
}
