pragma solidity ^0.7.0;

contract Test {
    function a(uint x) public {
      new Test2(x);
    }
}
contract Test2 {
    constructor(uint x) public {
      x+1;
    }
}
