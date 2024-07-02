pragma solidity >0.4.99 <0.8.0;

contract Account {
  address public owner;

  constructor(address payable _owner) public {
    owner = _owner;
  }

  function setOwner(address _owner) public {
    require(msg.sender == owner);
    owner = _owner;
  }

  function destroy(address payable recipient) public {
    require(msg.sender == owner);
    selfdestruct(recipient);
  }
}

contract Factory {

    bytes32 private contractCodeHash;

    constructor() public {
        contractCodeHash = keccak256(
            type(Account).creationCode
        );
    }
}
