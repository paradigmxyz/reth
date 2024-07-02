pragma solidity ^0.6.0;

contract B_Wallet {

    event Deposit(address indexed _sender, uint _value, bytes data);

    receive() external payable
    {
        if (msg.value > 0)
            emit Deposit(msg.sender, msg.value, msg.data);
    }

    function transferPayment(uint payment, address payable recipient) public {
        recipient.transfer(payment);
    }

    function sendPayment(uint payment, address payable recipient) public {
        require(recipient.send(payment), 'sendPayment failed');
    }

    function getBalance() public view returns(uint){
        return address(this).balance;
    }
}
