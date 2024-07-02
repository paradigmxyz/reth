pragma solidity >=0.4.22 <0.8.0;

contract Test {
  mapping (address => uint) balances;

  event Transfer(address indexed _from, address indexed _to, uint256 _value);

  constructor() public {
    balances[tx.origin] = 10000;
  }

  function sendCoin(address receiver, uint amount) public returns(bool sufficient) {
    if (balances[msg.sender] < amount)
      return false;
    else if (amount == 1)
      return false;
    else if (amount == 2 || amount == 3 || amount == 4)
      return false;
    else if (amount == 5)
      return false;
    else if (amount == 1)
      return false;
    else if (amount == 2 || amount == 3 || amount == 4)
      return false;
    else if (amount == 5)
      return false;
    else if (amount == 1)
      return false;
    else if (amount == 2 || amount == 3 || amount == 4)
      return false;
    else if (amount == 5)
      return false;
    else if (amount == 1)
      return false;
    else if (amount == 2 || amount == 3 || amount == 4)
      return false;
    else if (amount == 5)
      return false;
    else if (amount == 1)
      return false;
    else if (amount == 2 || amount == 3 || amount == 4)
      return false;
    else if (amount == 5)
      return false;

    balances[msg.sender] -= amount;
    balances[receiver] += amount;
    emit Transfer(msg.sender, receiver, amount);

    if(balances[receiver] >= amount)
      sufficient = true;
  }

  function getBalance(address addr) public view returns(uint) {
    return balances[addr];
  }
}

