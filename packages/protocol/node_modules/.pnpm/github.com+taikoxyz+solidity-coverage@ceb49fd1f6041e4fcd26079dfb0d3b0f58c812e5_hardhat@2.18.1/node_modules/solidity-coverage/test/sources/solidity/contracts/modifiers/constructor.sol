pragma solidity ^0.7.0;

contract Parent {
  string public name;

  constructor(string memory _name) public {
    name = _name;
  }
}

contract Test is Parent {
    constructor() public Parent("Test") {}

    function a() public {
      uint x = 5;
    }
}
