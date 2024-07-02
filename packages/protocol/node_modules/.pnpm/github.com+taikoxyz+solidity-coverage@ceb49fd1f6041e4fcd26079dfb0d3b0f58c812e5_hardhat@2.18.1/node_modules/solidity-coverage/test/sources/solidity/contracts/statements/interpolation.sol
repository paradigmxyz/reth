pragma solidity ^0.7.0;

contract Interpolated {
    constructor(string memory a) public {
      string memory b = a;
    }
}

contract TestA is Interpolated("abc{defg}"){
    function a(uint x) public {
        uint y = x;
    }
}

contract TestB is Interpolated {
    constructor(uint x) public Interpolated("abc{defg}") {
        uint y = x;
    }
}
