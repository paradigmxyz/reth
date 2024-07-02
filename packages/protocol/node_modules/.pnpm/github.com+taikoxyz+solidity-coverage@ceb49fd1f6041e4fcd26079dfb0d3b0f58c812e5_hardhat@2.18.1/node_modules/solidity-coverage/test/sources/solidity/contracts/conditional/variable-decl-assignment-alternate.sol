pragma solidity ^0.7.0;

contract Test {
    function a() public {
        bool x = false;
        bool y = false;
        bool z = (x) ? false : true;
    }
}
