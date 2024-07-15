// SPDX-License-Identifier: MIT
pragma solidity 0.8.24;

import "@openzeppelin/contracts/proxy/utils/UUPSUpgradeable.sol";
import "@openzeppelin/contracts-upgradeable/access/Ownable2StepUpgradeable.sol";
import "./AddressResolver.sol";
import "../libs/LibNetwork.sol";

/// @title EssentialContract
/// @custom:security-contact security@taiko.xyz
abstract contract EssentialContract is UUPSUpgradeable, Ownable2StepUpgradeable, AddressResolver {
    uint8 private constant _FALSE = 1;

    uint8 private constant _TRUE = 2;

    /// @dev The slot in transient storage of the reentry lock.
    /// This is the result of keccak256("ownerUUPS.reentry_slot") plus 1. The addition aims to
    /// prevent hash collisions with slots defined in EIP-1967, where slots are derived by
    /// keccak256("something") - 1, and with slots in SignalService, calculated directly with
    /// keccak256("something").
    bytes32 private constant _REENTRY_SLOT =
        0xa5054f728453d3dbe953bdc43e4d0cb97e662ea32d7958190f3dc2da31d9721b;

    /// @dev Slot 1.
    uint8 private __reentry;
    uint8 private __paused;
    uint64 public lastUnpausedAt;

    uint256[49] private __gap;

    /// @notice Emitted when the contract is paused.
    /// @param account The account that paused the contract.
    event Paused(address account);

    /// @notice Emitted when the contract is unpaused.
    /// @param account The account that unpaused the contract.
    event Unpaused(address account);

    error INVALID_PAUSE_STATUS();
    error FUNC_NOT_IMPLEMENTED();
    error REENTRANT_CALL();
    error ZERO_ADDRESS();
    error ZERO_VALUE();

    /// @dev Modifier that ensures the caller is the owner or resolved address of a given name.
    /// @param _name The name to check against.
    modifier onlyFromOwnerOrNamed(bytes32 _name) {
        if (msg.sender != owner() && msg.sender != resolve(_name, true)) revert RESOLVER_DENIED();
        _;
    }

    modifier notImplemented() {
        revert FUNC_NOT_IMPLEMENTED();
        _;
    }

    modifier nonReentrant() {
        if (_loadReentryLock() == _TRUE) revert REENTRANT_CALL();
        _storeReentryLock(_TRUE);
        _;
        _storeReentryLock(_FALSE);
    }

    modifier whenPaused() {
        if (!paused()) revert INVALID_PAUSE_STATUS();
        _;
    }

    modifier whenNotPaused() {
        if (paused()) revert INVALID_PAUSE_STATUS();
        _;
    }

    modifier nonZeroAddr(address _addr) {
        if (_addr == address(0)) revert ZERO_ADDRESS();
        _;
    }

    modifier nonZeroValue(bytes32 _value) {
        if (_value == 0) revert ZERO_VALUE();
        _;
    }

    /// @custom:oz-upgrades-unsafe-allow constructor
    constructor() {
        _disableInitializers();
    }

    /// @notice Pauses the contract.
    function pause() public virtual {
        _pause();
        // We call the authorize function here to avoid:
        // Warning (5740): Unreachable code.
        _authorizePause(msg.sender, true);
    }

    /// @notice Unpauses the contract.
    function unpause() public virtual {
        _unpause();
        // We call the authorize function here to avoid:
        // Warning (5740): Unreachable code.
        _authorizePause(msg.sender, false);
    }

    function impl() public view returns (address) {
        return _getImplementation();
    }

    /// @notice Returns true if the contract is paused, and false otherwise.
    /// @return true if paused, false otherwise.
    function paused() public view returns (bool) {
        return __paused == _TRUE;
    }

    function inNonReentrant() public view returns (bool) {
        return _loadReentryLock() == _TRUE;
    }

    /// @notice Initializes the contract.
    /// @param _owner The owner of this contract. msg.sender will be used if this value is zero.
    /// @param _addressManager The address of the {AddressManager} contract.
    function __Essential_init(
        address _owner,
        address _addressManager
    )
        internal
        nonZeroAddr(_addressManager)
    {
        __Essential_init(_owner);
        __AddressResolver_init(_addressManager);
    }

    function __Essential_init(address _owner) internal virtual onlyInitializing {
        __Context_init();
        _transferOwnership(_owner == address(0) ? msg.sender : _owner);
        __paused = _FALSE;
    }

    function _pause() internal whenNotPaused {
        __paused = _TRUE;
        emit Paused(msg.sender);
    }

    function _unpause() internal whenPaused {
        __paused = _FALSE;
        lastUnpausedAt = uint64(block.timestamp);
        emit Unpaused(msg.sender);
    }

    function _authorizeUpgrade(address) internal virtual override onlyOwner { }

    function _authorizePause(address, bool) internal virtual onlyOwner { }

    // Stores the reentry lock
    function _storeReentryLock(uint8 _reentry) internal virtual {
        if (LibNetwork.isDencunSupported(block.chainid)) {
            assembly {
                tstore(_REENTRY_SLOT, _reentry)
            }
        } else {
            __reentry = _reentry;
        }
    }

    // Loads the reentry lock
    function _loadReentryLock() internal view virtual returns (uint8 reentry_) {
        if (LibNetwork.isDencunSupported(block.chainid)) {
            assembly {
                reentry_ := tload(_REENTRY_SLOT)
            }
        } else {
            reentry_ = __reentry;
        }
    }
}
