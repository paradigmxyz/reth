pragma solidity ^0.7.0;


contract ContractB {
  uint x;
  constructor() public {
  }

  function sendFnB() public {
    x = 5;
  }

  function callFnB() public pure returns (uint){
    uint y = 5;
    return y;
  }
}
