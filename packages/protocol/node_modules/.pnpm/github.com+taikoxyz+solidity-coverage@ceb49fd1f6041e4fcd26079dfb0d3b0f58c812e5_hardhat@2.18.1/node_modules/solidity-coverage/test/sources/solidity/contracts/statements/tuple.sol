pragma solidity ^0.7.0;

contract Test {

    function returnTuple() public returns (uint x, uint y) {
        return (10, 20);
    }

    function a() public {
        (uint _a, uint _b) = (10, 20);
        (_a, _b) = returnTuple();
    }
}
