// SPDX-License-Identifier: MIT
//  _____     _ _         _         _
// |_   _|_ _(_) |_____  | |   __ _| |__ ___
//   | |/ _` | | / / _ \ | |__/ _` | '_ (_-<
//   |_|\__,_|_|_\_\___/ |____\__,_|_.__/__/

pragma solidity ^0.8.20;

import "lib/openzeppelin-contracts/contracts/utils/Strings.sol";
import "./AddressManager.sol";

/// @title AddressResolver
/// @notice This contract acts as a bridge for name-to-address resolution.
/// It delegates the resolution to the AddressManager. By separating the logic,
/// we can maintain flexibility in address management without affecting the
/// resolving process.
///
/// Note that the address manager should be changed using upgradability, there
/// is no setAddressManager() function go guarantee atomicness across all
/// contracts that are resolvers.
abstract contract AddressResolver {
    using Strings for uint256;

    address public addressManager;
    uint256[49] private __gap;

    error RESOLVER_DENIED();
    error RESOLVER_INVALID_MANAGER();
    error RESOLVER_UNEXPECTED_CHAINID();
    error RESOLVER_ZERO_ADDR(uint64 chainId, string name);

    /// @dev Modifier that ensures the caller is the resolved address of a given
    /// name.
    /// @param name The name to check against.
    modifier onlyFromNamed(bytes32 name) {
        if (msg.sender != resolve(name, true)) revert RESOLVER_DENIED();
        _;
    }

    /// @notice Resolves a name to its address deployed on this chain.
    /// @param name Name whose address is to be resolved.
    /// @param allowZeroAddress If set to true, does not throw if the resolved
    /// address is `address(0)`.
    /// @return addr Address associated with the given name.
    function resolve(
        bytes32 name,
        bool allowZeroAddress
    )
        public
        view
        virtual
        returns (address payable addr)
    {
        return _resolve(uint64(block.chainid), name, allowZeroAddress);
    }

    /// @notice Resolves a name to its address deployed on a specified chain.
    /// @param chainId The chainId of interest.
    /// @param name Name whose address is to be resolved.
    /// @param allowZeroAddress If set to true, does not throw if the resolved
    /// address is `address(0)`.
    /// @return addr Address associated with the given name on the specified
    /// chain.
    function resolve(
        uint64 chainId,
        bytes32 name,
        bool allowZeroAddress
    )
        public
        view
        virtual
        returns (address payable addr)
    {
        return _resolve(chainId, name, allowZeroAddress);
    }

    /// @dev Initialization method for setting up AddressManager reference.
    /// @param _addressManager Address of the AddressManager.
    // solhint-disable-next-line func-name-mixedcase
    function __AddressResolver_init(address _addressManager) internal virtual {
        if (block.chainid >= type(uint64).max) {
            revert RESOLVER_UNEXPECTED_CHAINID();
        }
        addressManager = _addressManager;
    }

    /// @dev Helper method to resolve name-to-address.
    /// @param chainId The chainId of interest.
    /// @param name Name whose address is to be resolved.
    /// @param allowZeroAddress If set to true, does not throw if the resolved
    /// address is `address(0)`.
    /// @return addr Address associated with the given name on the specified
    /// chain.
    function _resolve(
        uint64 chainId,
        bytes32 name,
        bool allowZeroAddress
    )
        private
        view
        returns (address payable addr)
    {
        if (addressManager == address(0)) revert RESOLVER_INVALID_MANAGER();

        addr = payable(IAddressManager(addressManager).getAddress(chainId, name));

        if (!allowZeroAddress && addr == address(0)) {
            revert RESOLVER_ZERO_ADDR(chainId, uint256(name).toString());
        }
    }
}
