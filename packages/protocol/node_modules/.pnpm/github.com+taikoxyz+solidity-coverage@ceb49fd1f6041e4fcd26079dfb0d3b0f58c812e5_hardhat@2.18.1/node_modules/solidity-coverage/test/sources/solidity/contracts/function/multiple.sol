pragma solidity ^0.7.0;

contract Test {
    function f1(uint x) public {
        x = 1;
    }

    function f2(uint x) public { x = 2; }

    address a;

    function f3(uint y) public {
        y = 1;
    }
}
