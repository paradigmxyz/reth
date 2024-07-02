pragma solidity ^0.7.0;

contract Test {
    modifier b(){
      uint y;
      _;
    }
    function a(uint x) b public {
        x;
    }
}
