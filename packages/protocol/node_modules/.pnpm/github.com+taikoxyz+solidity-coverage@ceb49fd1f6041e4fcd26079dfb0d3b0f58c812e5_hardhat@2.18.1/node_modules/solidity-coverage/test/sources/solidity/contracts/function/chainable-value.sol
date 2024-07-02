pragma solidity ^0.7.0;

// This is for a test that verifies solcover can instrument a
// another kind of long CallExpression chain
contract Test {
    function paySomeone(address x, address y) public payable {
    }

    function a() public payable {
        Test(0x00).paySomeone{value: msg.value}(0x0000000000000000000000000000000000000000, 0x0000000000000000000000000000000000000000);
    }
}
