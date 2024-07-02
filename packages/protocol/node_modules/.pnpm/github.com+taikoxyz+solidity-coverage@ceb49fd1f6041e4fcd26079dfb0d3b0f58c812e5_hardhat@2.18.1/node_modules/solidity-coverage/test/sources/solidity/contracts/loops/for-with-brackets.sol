pragma solidity ^0.7.0;

contract Test {
    function a() public {
        for(uint x = 0; x < 10; x++){
            keccak256(abi.encodePacked(x));
        }
    }
}
