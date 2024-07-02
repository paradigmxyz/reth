pragma solidity ^0.6.0;

import "./ModifiersB.sol";

/**
 * New syntaxes in solc 0.6.x
 */
contract ModifiersC {
  uint counter;
  address owner;
  bool flag = true;

  constructor() public {
    owner = msg.sender;
  }

  modifier flippable {
    require(flag);
    _;
  }

  function flip() public {
    flag = !flag;
  }

  function simpleSetFlip(uint i) flippable public {
    counter = counter + i;
  }

  modifier onlyOwner {
    require(msg.sender == owner);
    _;
  }

  function set(uint i)
    onlyOwner
    public
    payable
    virtual
  {
    counter = counter + i;
  }
}
