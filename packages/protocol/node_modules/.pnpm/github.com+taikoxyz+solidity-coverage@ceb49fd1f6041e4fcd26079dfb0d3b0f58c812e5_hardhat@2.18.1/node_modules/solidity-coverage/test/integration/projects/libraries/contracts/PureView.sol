pragma solidity ^0.7.0;

contract PureView {

  // Make sure we aren't corrupting anything with the replace
  uint notpureview = 5;

  function inheritedPure(uint a, uint b) public pure returns(uint){
    return a + b;
  }

  function inheritedView() public view returns (uint){
    return notpureview;
  }
}