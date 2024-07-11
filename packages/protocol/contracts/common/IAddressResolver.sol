// SPDX-License-Identifier: MIT
pragma solidity 0.8.24;

/// @title IAddressResolver
/// @notice This contract acts as a bridge for name-to-address resolution.
/// It delegates the resolution to the AddressManager. By separating the logic,
/// we can maintain flexibility in address management without affecting the
/// resolving process.
/// @dev Note that the address manager should be changed using upgradability, there
/// is no setAddressManager() function to guarantee atomicity across all
/// contracts that are resolvers.
/// @custom:security-contact security@taiko.xyz
interface IAddressResolver {
    /// @notice Resolves a name to its address deployed on this chain.
    /// @param _name Name whose address is to be resolved.
    /// @param _allowZeroAddress If set to true, does not throw if the resolved
    /// address is `address(0)`.
    /// @return Address associated with the given name.
    function resolve(
        bytes32 _name,
        bool _allowZeroAddress
    )
        external
        view
        returns (address payable);

    /// @notice Resolves a name to its address deployed on a specified chain.
    /// @param _chainId The chainId of interest.
    /// @param _name Name whose address is to be resolved.
    /// @param _allowZeroAddress If set to true, does not throw if the resolved
    /// address is `address(0)`.
    /// @return Address associated with the given name on the specified
    /// chain.
    function resolve(
        uint64 _chainId,
        bytes32 _name,
        bool _allowZeroAddress
    )
        external
        view
        returns (address payable);
}
