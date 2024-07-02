pragma solidity ^0.7.0;


contract MatrixA {
  uint x;
  constructor() public {
  }

  function sendFn() public {
    x = 5;
  }

  function callFn() public pure returns (uint){
    uint y = 5;
    return y;
  }

  function unhit() public {
    uint z = 7;
  }
}
