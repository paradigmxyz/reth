pragma solidity ^0.6.0;

import "./ModifiersB.sol";

/**
 * New syntaxes in solc 0.6.x
 */
contract ModifiersA is ModifiersB {
  uint counter;
  bool flag = true;

  modifier flippable {
    require(flag);
    _;
  }

  modifier overridden() override {
    require(true);
    _;
  }

  function flip() public {
    flag = !flag;
  }

  function simpleSet(uint i)
    public
    override(ModifiersB)
  {
    counter = counter + i;
  }

  function simpleView(uint i)
    view
    overridden
    external
    returns (uint, bool)
  {
    return (counter + i, true);
  }

  function simpleSetFlip(uint i) flippable public {
    counter = counter + i;
  }
}
