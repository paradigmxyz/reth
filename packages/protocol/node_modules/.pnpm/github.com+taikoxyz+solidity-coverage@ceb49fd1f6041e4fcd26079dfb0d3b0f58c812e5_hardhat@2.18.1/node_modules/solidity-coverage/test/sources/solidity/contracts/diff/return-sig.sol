pragma solidity ^0.7.0;

contract Old {
  function a() public view returns (uint) {
    return 1;
  }
}

contract New {
  function a() public view returns (bool) {
    return true;
  }

  function e() public view returns (uint8[2] memory) {
    return [5,7];
  }

  function f() public view returns (uint8[2] memory, uint) {
    return ([5,7], 7);
  }

  function g() public view returns (uint8[3] memory) {
    return [5,7,8];
  }
}
