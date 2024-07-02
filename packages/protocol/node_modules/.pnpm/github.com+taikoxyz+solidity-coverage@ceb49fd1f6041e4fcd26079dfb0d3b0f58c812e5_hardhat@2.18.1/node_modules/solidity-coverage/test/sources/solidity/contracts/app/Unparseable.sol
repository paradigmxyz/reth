pragma solidity ^0.7.0;

contract Unparseable {
    uint x = 0;

    function test(uint val) public {
        x = x + val;
    }

    function getX() public view returns (uint){
        return x;

    // Missing a bracket!
}
