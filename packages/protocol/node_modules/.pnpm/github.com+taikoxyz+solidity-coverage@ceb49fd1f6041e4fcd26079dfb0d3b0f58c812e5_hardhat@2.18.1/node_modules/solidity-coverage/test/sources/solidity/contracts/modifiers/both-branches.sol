pragma solidity ^0.7.0;

contract Test {
    bool flag = true;

    modifier m {
      require(flag);
      _;
    }

    function flip() public {
      flag = !flag;
    }

    function a() m public {
      uint x = 5;
    }
}
