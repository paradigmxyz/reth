pragma solidity ^0.7.4;
pragma experimental ABIEncoderV2;

address constant LENDING_POOL = 0xB53C1a33016B2DC2fF3653530bfF1848a515c8c5;
address constant CHAINLINK = 0x5f4eC3Df9cbd43714FE2740f5E3616155c5b8419;

function _addFive(uint x)
  pure
  returns (uint)
{
  return x + 5;
}

function _addSeven(uint x)
  pure
  returns (uint)
{
  return x + 7;
}
