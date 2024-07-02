pragma solidity ^0.7.4;
pragma abicoder v2;

import {
  LENDING_POOL,
  CHAINLINK,
  _addFive,
  _addSeven
} from "./Functions_solc7.sol";

function _addTen(uint x)
  pure
  returns (uint)
{
  return x + 10;
}

/**
 * New syntaxes in solc 0.7.x
 */
contract ContractA {
  uint y = 5;

  function addFive()
    public
    view
    returns (uint)
  {
    return _addFive(y);
  }

  function addSeven()
    public
    view
    returns (uint)
  {
    return _addSeven(y);
  }
}

contract ContractB {
  uint y = 5;

  function addTen()
    public
    view
    returns (uint)
  {
    return _addTen(y);
  }

}
