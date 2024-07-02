pragma solidity ^0.7.0;

contract Test {
    modifier m(string memory val) {
      _;
    }

    function a()
      m('ETH')
      m('BTC')
      public
    {
      uint x = 5;
    }
}
