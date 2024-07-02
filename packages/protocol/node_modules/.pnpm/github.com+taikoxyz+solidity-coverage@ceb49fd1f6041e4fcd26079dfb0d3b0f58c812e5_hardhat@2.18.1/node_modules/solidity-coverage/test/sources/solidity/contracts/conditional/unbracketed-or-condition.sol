pragma solidity ^0.7.0;

contract Test {
    function a() public {
        bool x = false;
        bool y = true;
        x || y ? y = false : y = false;
    }
}
