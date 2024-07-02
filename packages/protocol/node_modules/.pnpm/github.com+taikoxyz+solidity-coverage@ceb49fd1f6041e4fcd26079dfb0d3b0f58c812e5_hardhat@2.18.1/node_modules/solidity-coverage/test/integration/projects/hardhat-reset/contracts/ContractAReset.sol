pragma solidity ^0.7.0;


contract ContractA {
  uint x;
  constructor() public {
  }

  function sendFn() public {
    x = 1;
  }

  function sendFn2() public {
    x = 2;
  }

  function callFn() public pure returns (uint){
    uint y = 5;
    return y;
  }

  function callFn2() public pure returns (uint){
    uint y = 5;
    return y;
  }
}
