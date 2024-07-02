pragma solidity ^0.7.0;
// This is for a test that verifies solcover can instrument a
// chained constructor/method call invoked by the new operator.
contract Chainable {
    function chainWith(uint y, uint z) public {}
}
contract Test {
    function a() public {
        new Chainable().chainWith(3, 4);
    }
}
