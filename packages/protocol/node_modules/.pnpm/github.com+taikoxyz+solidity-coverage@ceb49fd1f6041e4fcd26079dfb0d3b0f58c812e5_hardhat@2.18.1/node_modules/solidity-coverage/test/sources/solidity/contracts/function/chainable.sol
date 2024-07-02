pragma solidity ^0.7.0;

// This is for a test that verifies solcover can instrument a
// chained constructor/method call.
contract Test {
    function chainWith(uint y, uint z) public {}

    function a() public {
        Test(0x00).chainWith(3, 4);
    }
}
