pragma solidity ^0.7.0;

contract Test {
    modifier m {
      require(false);
      _;
    }

    function a() m public {
      uint x = 5;
    }
}
