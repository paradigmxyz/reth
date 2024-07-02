pragma solidity ^0.7.0;

contract Old {
  function a() public {
    bool x = true;
  }
}

contract New {
  function a() public view returns (bool) {
    return true;
  }
}
