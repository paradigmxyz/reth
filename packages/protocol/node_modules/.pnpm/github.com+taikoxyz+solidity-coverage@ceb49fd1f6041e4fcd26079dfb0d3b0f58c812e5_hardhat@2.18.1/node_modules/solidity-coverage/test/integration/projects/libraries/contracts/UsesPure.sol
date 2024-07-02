pragma solidity ^0.7.0;

import "./_Interface.sol";
import "./PureView.sol";
import "./CLibrary.sol";

contract UsesPure is PureView, _Interface {
  uint onehundred = 99;

  function usesThem() public view {
    uint y = isPure(1,2);
    uint z = isView();
  }

  function isPure(uint a, uint b) public pure returns (uint){
    return a * b;
  }

  function isView() public view returns (uint){
    return notpureview;
  }

  function isConstant() public view returns (uint){
    return onehundred;
  }

  function bePure(uint a, uint b) public pure returns (uint) {
    return a + b;
  }

  function beView() public view returns (uint){
    return onehundred;
  }

  function usesLibrary() public view returns (uint){
    return CLibrary.a();
  }

  function multiline(uint x,
                     uint y)
                     public
                     view
                     returns (uint)
  {
    return onehundred;
  }

  function stare(uint a, uint b) external override {
    uint z = a + b;
  }

  function cry() external override {

  }
}