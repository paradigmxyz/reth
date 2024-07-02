pragma solidity ^0.7.0;

contract Old {
  uint y;

  function a() public {
    bool x = true;
  }

  function b() external {
    bool x = true;
  }

  function c() external {
    bool x = true;
  }
}

contract New {
  uint y;

  function a() public {
    bool x = true;
  }

  function b(bytes8 z) external {
    bool x = true;
  }

  function c(uint q, uint r) external {
    bool x = true;
  }
}
