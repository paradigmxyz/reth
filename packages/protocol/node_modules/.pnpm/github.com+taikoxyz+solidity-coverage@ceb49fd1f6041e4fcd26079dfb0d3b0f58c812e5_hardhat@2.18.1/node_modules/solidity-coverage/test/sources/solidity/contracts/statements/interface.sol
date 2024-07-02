pragma solidity ^0.7.0;

interface IInterface {
    event Assign(address indexed token, address indexed from, address indexed to, uint256 amount);
    event Withdraw(address indexed token, address indexed from, address indexed to, uint256 amount);

    // TODO: remove init from the interface, all the initialization should be outside the court
    function init(address _owner) external;

    function assign(uint _token, address _to, uint256 _amount) external;
    function withdraw(uint _token, address _to, uint256 _amount) external;
}
