// SPDX-License-Identifier: MIT
pragma solidity 0.8.24;

import "@openzeppelin/contracts-upgradeable/proxy/utils/Initializable.sol";
import "./IAddressManager.sol";
import "./IAddressResolver.sol";

/// @title AddressResolver
/// @notice See the documentation in {IAddressResolver}.
/// @custom:security-contact security@taiko.xyz
abstract contract AddressResolver is IAddressResolver, Initializable {
    /// @notice Address of the AddressManager.
    address public addressManager;
    uint256[49] private __gap;

    error RESOLVER_DENIED();
    error RESOLVER_INVALID_MANAGER();
    error RESOLVER_UNEXPECTED_CHAINID();
    error RESOLVER_ZERO_ADDR(uint64 chainId, bytes32 name);

    /// @dev Modifier that ensures the caller is the resolved address of a given
    /// name.
    /// @param _name The name to check against.
    modifier onlyFromNamed(bytes32 _name) {
        if (msg.sender != resolve(_name, true)) revert RESOLVER_DENIED();
        _;
    }

    /// @dev Modifier that ensures the caller is a resolved address to either _name1 or _name2
    /// name.
    /// @param _name1 The first name to check against.
    /// @param _name2 The second name to check against.
    modifier onlyFromNamedEither(bytes32 _name1, bytes32 _name2) {
        if (msg.sender != resolve(_name1, true) && msg.sender != resolve(_name2, true)) {
            revert RESOLVER_DENIED();
        }
        _;
    }

    /// @custom:oz-upgrades-unsafe-allow constructor
    constructor() {
        _disableInitializers();
    }

    /// @inheritdoc IAddressResolver
    function resolve(
        bytes32 _name,
        bool _allowZeroAddress
    )
        public
        view
        virtual
        returns (address payable)
    {
        return _resolve(uint64(block.chainid), _name, _allowZeroAddress);
    }

    /// @inheritdoc IAddressResolver
    function resolve(
        uint64 _chainId,
        bytes32 _name,
        bool _allowZeroAddress
    )
        public
        view
        virtual
        returns (address payable)
    {
        return _resolve(_chainId, _name, _allowZeroAddress);
    }

    /// @dev Initialization method for setting up AddressManager reference.
    /// @param _addressManager Address of the AddressManager.
    function __AddressResolver_init(address _addressManager) internal virtual onlyInitializing {
        if (block.chainid > type(uint64).max) {
            revert RESOLVER_UNEXPECTED_CHAINID();
        }
        addressManager = _addressManager;
    }

    /// @dev Helper method to resolve name-to-address.
    /// @param _chainId The chainId of interest.
    /// @param _name Name whose address is to be resolved.
    /// @param _allowZeroAddress If set to true, does not throw if the resolved
    /// address is `address(0)`.
    /// @return addr_ Address associated with the given name on the specified
    /// chain.
    function _resolve(
        uint64 _chainId,
        bytes32 _name,
        bool _allowZeroAddress
    )
        private
        view
        returns (address payable addr_)
    {
        address _addressManager = addressManager;
        if (_addressManager == address(0)) revert RESOLVER_INVALID_MANAGER();

        addr_ = payable(IAddressManager(_addressManager).getAddress(_chainId, _name));

        if (!_allowZeroAddress && addr_ == address(0)) {
            revert RESOLVER_ZERO_ADDR(_chainId, _name);
        }
    }
}
