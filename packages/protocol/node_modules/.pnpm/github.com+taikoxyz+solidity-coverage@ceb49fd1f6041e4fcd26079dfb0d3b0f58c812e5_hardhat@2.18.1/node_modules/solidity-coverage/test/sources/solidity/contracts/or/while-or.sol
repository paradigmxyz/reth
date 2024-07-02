pragma solidity ^0.7.0;

contract Test {
    function a(uint x) public {
        uint counter;
        while( (x == 1 || x == 2) && counter < 2 ){
            counter++;
        }
    }
}
