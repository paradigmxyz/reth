pragma solidity ^0.7.0;

contract Test {
    function a(uint x) public {
        if (x == 1) {
            revert();
        } else if (x == 50)
          x = 5;
    }
}
