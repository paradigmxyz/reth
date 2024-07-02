pragma solidity ^0.7.0;

contract Old {
  uint y;

  event Evt(uint x, bytes8 y);

  function a() public {
    bool x = true;
  }
}

contract New {
  uint y;

  function a() public {
    bool x = true;
  }

  event aEvt(bytes8);
  event _Evt(bytes8 x, bytes8 y);
}

