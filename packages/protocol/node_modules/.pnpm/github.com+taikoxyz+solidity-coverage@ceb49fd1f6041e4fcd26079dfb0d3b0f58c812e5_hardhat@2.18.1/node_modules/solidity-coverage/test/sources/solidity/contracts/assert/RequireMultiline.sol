pragma solidity ^0.7.0;

contract Test {
  function a(bool _a, bool _b, bool _c) public {
    require(_a &&
            _b &&
            _c);
  }
}
