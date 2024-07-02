pragma solidity ^0.7.0;

contract Test {
    modifier mmm {
      require(true);
      _;
    }

    function a() mmm public {
      uint x = 5;
    }

    function b() mmm public {
      uint x = 5;
    }
}
