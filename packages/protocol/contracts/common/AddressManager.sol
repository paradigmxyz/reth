// SPDX-License-Identifier: MIT
//  _____     _ _         _         _
// |_   _|_ _(_) |_____  | |   __ _| |__ ___
//   | |/ _` | | / / _ \ | |__/ _` | '_ (_-<
//   |_|\__,_|_|_\_\___/ |____\__,_|_.__/__/

pragma solidity ^0.8.20;

import "./OwnerUUPSUpgradable.sol";
/// @title IAddressManager
/// @notice Specifies methods to manage address mappings for given chainId-name
/// pairs.

interface IAddressManager {
    /// @notice Gets the address mapped to a specific chainId-name pair.
    /// @dev Note that in production, this method shall be a pure function
    /// without any storage access.
    /// @param chainId The chainId for which the address needs to be fetched.
    /// @param name The name for which the address needs to be fetched.
    /// @return Address associated with the chainId-name pair.
    function getAddress(uint64 chainId, bytes32 name) external view returns (address);
}

/// @title AddressManager
/// @notice Manages a mapping of chainId-name pairs to Ethereum addresses.
contract AddressManager is OwnerUUPSUpgradable, IAddressManager {
    mapping(uint256 => mapping(bytes32 => address)) private addresses;
    uint256[49] private __gap;

    event AddressSet(
        uint64 indexed chainId, bytes32 indexed name, address newAddress, address oldAddress
    );

    /// @notice Initializes the owner for the upgradable contract.
    function init() external initializer {
        __OwnerUUPSUpgradable_init();
    }

    /// @notice Sets the address for a specific chainId-name pair.
    /// @param chainId The chainId to which the address will be mapped.
    /// @param name The name to which the address will be mapped.
    /// @param newAddress The Ethereum address to be mapped.
    function setAddress(
        uint64 chainId,
        bytes32 name,
        address newAddress
    )
        external
        virtual
        onlyOwner
    {
        address oldAddress = addresses[chainId][name];
        addresses[chainId][name] = newAddress;
        emit AddressSet(chainId, name, newAddress, oldAddress);
    }

    /// @inheritdoc IAddressManager
    function getAddress(uint64 chainId, bytes32 name) public view override returns (address) {
        return addresses[chainId][name];
    }
}
