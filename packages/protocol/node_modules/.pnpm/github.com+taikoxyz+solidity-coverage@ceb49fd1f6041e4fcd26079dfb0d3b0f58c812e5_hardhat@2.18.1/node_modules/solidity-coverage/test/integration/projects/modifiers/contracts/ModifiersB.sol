pragma solidity ^0.6.0;


contract ModifiersB {
  uint value;
  uint b;

  constructor() public {
  }

  modifier overridden() virtual {
    require(true);
    _;
  }

  function simpleSet(uint i) public virtual {
    value = 5;
  }
}
