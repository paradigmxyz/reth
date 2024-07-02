pragma solidity ^0.5.0;


contract ContractB {
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
}
