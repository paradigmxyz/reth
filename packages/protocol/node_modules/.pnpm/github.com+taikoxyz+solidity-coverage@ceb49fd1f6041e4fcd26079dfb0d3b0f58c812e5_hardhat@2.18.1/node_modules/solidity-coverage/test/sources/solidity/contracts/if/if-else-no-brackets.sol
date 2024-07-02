pragma solidity ^0.7.0;

contract Test {
  function a(uint x,uint y, uint z) public {
    if (x==y)
      x = 5;
    else
      x = 7;
  }
}
