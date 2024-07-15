// SPDX-License-Identifier: MIT
pragma solidity 0.8.24;

import "./EssentialContract.sol";

/// @title AddressManager
/// @notice See the documentation in {IAddressManager}.
/// @custom:security-contact security@taiko.xyz
contract AddressManager is EssentialContract, IAddressManager {
    /// @dev Mapping of chainId to mapping of name to address.
    mapping(uint256 chainId => mapping(bytes32 name => address addr)) private __addresses;

    uint256[49] private __gap;

    /// @notice Emitted when an address is set.
    /// @param chainId The chainId for the address mapping.
    /// @param name The name for the address mapping.
    /// @param newAddress The new address.
    /// @param oldAddress The old address.
    event AddressSet(
        uint64 indexed chainId, bytes32 indexed name, address newAddress, address oldAddress
    );

    error AM_ADDRESS_ALREADY_SET();

    /// @notice Initializes the contract.
    /// @param _owner The owner of this contract. msg.sender will be used if this value is zero.
    function init(address _owner) external initializer {
        __Essential_init(_owner);
        addressManager = address(this);
    }

    function init2() external onlyOwner reinitializer(2) {
        addressManager = address(this);
    }

    /// @notice Sets the address for a specific chainId-name pair.
    /// @param _chainId The chainId to which the address will be mapped.
    /// @param _name The name to which the address will be mapped.
    /// @param _newAddress The Ethereum address to be mapped.
    function setAddress(
        uint64 _chainId,
        bytes32 _name,
        address _newAddress
    )
        external
        virtual
        onlyOwner
    {
        address oldAddress = __addresses[_chainId][_name];
        if (_newAddress == oldAddress) revert AM_ADDRESS_ALREADY_SET();
        __addresses[_chainId][_name] = _newAddress;
        emit AddressSet(_chainId, _name, _newAddress, oldAddress);
    }

    /// @inheritdoc IAddressManager
    function getAddress(uint64 _chainId, bytes32 _name) external view override returns (address) {
        address addr = _getOverride(_chainId, _name);
        if (addr != address(0)) return addr;
        else return __addresses[_chainId][_name];
    }

    /// @notice Gets the address mapped to a specific chainId-name pair.
    /// @dev Sub-contracts can override this method to avoid reading from storage.
    function _getOverride(uint64 _chainId, bytes32 _name) internal pure virtual returns (address) { }

    function _authorizePause(address, bool) internal pure override notImplemented { }
}
