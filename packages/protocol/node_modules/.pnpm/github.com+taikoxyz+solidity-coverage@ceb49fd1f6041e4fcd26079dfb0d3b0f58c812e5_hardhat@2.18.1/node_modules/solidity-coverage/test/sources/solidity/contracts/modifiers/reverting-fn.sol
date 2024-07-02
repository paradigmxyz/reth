pragma solidity ^0.7.0;

contract Test {
    bool flag = true;

    modifier m {
      require(flag);
      _;
    }

    function a(bool success) m public {
        require(success);
    }
}
