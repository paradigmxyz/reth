pragma solidity ^0.7.0;

contract Test {
    function a() public {
        bool x = true;
        bool y = false;
        (x) ? y = false : y = false;
    }
}
