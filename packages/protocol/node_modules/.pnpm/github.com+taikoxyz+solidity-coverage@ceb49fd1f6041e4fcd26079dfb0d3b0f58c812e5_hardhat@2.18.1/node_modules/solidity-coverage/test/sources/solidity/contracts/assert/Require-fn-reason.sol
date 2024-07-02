pragma solidity ^0.7.0;

contract Test {
  function getBool(bool _b) public pure returns (bool){
    return _b;
  }

  function a(bool _a) public {
    require(getBool(_a), "mi ritrovai per una selva oscura");
  }
}
