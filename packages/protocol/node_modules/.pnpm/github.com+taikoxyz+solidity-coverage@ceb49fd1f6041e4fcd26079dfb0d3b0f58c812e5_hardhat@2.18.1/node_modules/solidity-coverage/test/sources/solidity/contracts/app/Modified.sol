pragma solidity ^0.7.0;

contract Modified {
  uint counter;

  modifier m  {
    _;
  }

  // When modifier coverage is on, branch cov should be 50%
  // When off: 100%
  function set(uint i)
    m
    public
    payable
    virtual
  {
    counter = counter + i;
  }
}
