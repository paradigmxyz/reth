pragma solidity ^0.5.0;

import "./Deed.sol";

/**
 * @title Deed to hold ether in exchange for ownership of a node
 * @dev The deed can be controlled only by the registrar and can only send ether back to the owner.
 */
contract DeedImplementation is Deed {

    address payable constant burn = address(0xdead);

    address payable private _owner;
    address private _previousOwner;
    address private _registrar;

    uint private _creationDate;
    uint private _value;

    bool active;

    event OwnerChanged(address newOwner);
    event DeedClosed();

    modifier onlyRegistrar {
        require(msg.sender == _registrar);
        _;
    }

    modifier onlyActive {
        require(active);
        _;
    }

    constructor(address payable initialOwner) public payable {
        _owner = initialOwner;
        _registrar = msg.sender;
        _creationDate = now;
        active = true;
        _value = msg.value;
    }

    function setOwner(address payable newOwner) external onlyRegistrar {
        require(newOwner != address(0x0));
        _previousOwner = _owner;  // This allows contracts to check who sent them the ownership
        _owner = newOwner;
        emit OwnerChanged(newOwner);
    }

    function setRegistrar(address newRegistrar) external onlyRegistrar {
        _registrar = newRegistrar;
    }

    function setBalance(uint newValue, bool throwOnFailure) external onlyRegistrar onlyActive {
        // Check if it has enough balance to set the value
        require(_value >= newValue);
        _value = newValue;
        // Send the difference to the owner
        require(_owner.send(address(this).balance - newValue) || !throwOnFailure);
    }

    /**
     * @dev Close a deed and refund a specified fraction of the bid value
     *
     * @param refundRatio The amount*1/1000 to refund
     */
    function closeDeed(uint refundRatio) external onlyRegistrar onlyActive {
        active = false;
        require(burn.send(((1000 - refundRatio) * address(this).balance)/1000));
        emit DeedClosed();
        _destroyDeed();
    }

    /**
     * @dev Close a deed and refund a specified fraction of the bid value
     */
    function destroyDeed() external {
        _destroyDeed();
    }

    function owner() external view returns (address) {
        return _owner;
    }

    function previousOwner() external view returns (address) {
        return _previousOwner;
    }

    function value() external view returns (uint) {
        return _value;
    }

    function creationDate() external view returns (uint) {
        _creationDate;
    }

    function _destroyDeed() internal {
        require(!active);

        // Instead of selfdestruct(owner), invoke owner fallback function to allow
        // owner to log an event if desired; but owner should also be aware that
        // its fallback function can also be invoked by setBalance
        if (_owner.send(address(this).balance)) {
            selfdestruct(burn);
        }
    }
}
