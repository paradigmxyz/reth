pragma solidity ^0.7.0;

contract Test {
    function a(uint x) public {
        if (x == 1 && true || x == 2) {
            /* ignore */
        } else {
            revert();
        }
    }
}
