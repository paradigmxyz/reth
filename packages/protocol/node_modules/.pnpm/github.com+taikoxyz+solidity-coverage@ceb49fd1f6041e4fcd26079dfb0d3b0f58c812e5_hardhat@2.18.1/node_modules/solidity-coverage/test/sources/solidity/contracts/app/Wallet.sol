pragma solidity ^0.7.0;

contract Wallet {

    event Deposit(address indexed _sender, uint _value);

    function transferPayment(uint payment, address payable recipient) public {
        recipient.transfer(payment);
    }

    function sendPayment(uint payment, address payable recipient) public {
        require(recipient.send(payment), 'sendPayment failed');
    }

    function getBalance() public view returns(uint){
        return address(this).balance;
    }

    receive() external payable
    {
        if (msg.value > 0)
            emit Deposit(msg.sender, msg.value);
    }
}
