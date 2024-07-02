pragma solidity ^0.5.5;


contract ContractA {
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
