// SPDX-License-Identifier: MIT
pragma solidity 0.8.24;

import "@openzeppelin/contracts-upgradeable/token/ERC20/ERC20Upgradeable.sol";
import "@openzeppelin/contracts-upgradeable/utils/introspection/IERC165Upgradeable.sol";
import "../common/EssentialContract.sol";
import "../common/LibStrings.sol";
import "./IBridgedERC20.sol";
import "./LibBridgedToken.sol";

/// @title BridgedERC20
/// @notice An upgradeable ERC20 contract that represents tokens bridged from
/// another chain.
/// @custom:security-contact security@taiko.xyz
contract BridgedERC20 is
    EssentialContract,
    IBridgedERC20,
    IBridgedERC20Initializable,
    IBridgedERC20Migratable,
    IERC165Upgradeable,
    ERC20Upgradeable
{
    /// @dev Slot 1.
    address public srcToken;

    uint8 public __srcDecimals;

    /// @dev Slot 2.
    uint256 public srcChainId;

    /// @dev Slot 3.
    /// @notice The address of the contract to migrate tokens to or from.
    address public migratingAddress;

    /// @notice If true, signals migrating 'to', false if migrating 'from'.
    bool public migratingInbound;

    uint256[47] private __gap;

    /// @notice Emitted when the migration status is changed.
    /// @param addr The address migrating 'to' or 'from'.
    /// @param inbound If false then signals migrating 'from', true if migrating 'into'.
    event MigrationStatusChanged(address addr, bool inbound);

    /// @notice Emitted when tokens are migrated to the new bridged token.
    /// @param migratedTo The address of the bridged token.
    /// @param account The address of the account.
    /// @param amount The amount of tokens migrated.
    event MigratedTo(address indexed migratedTo, address indexed account, uint256 amount);

    /// @notice Emitted when tokens are migrated from the old bridged token.
    /// @param migratedFrom The address of the bridged token.
    /// @param account The address of the account.
    /// @param amount The amount of tokens migrated.
    event MigratedFrom(address indexed migratedFrom, address indexed account, uint256 amount);

    error BTOKEN_INVALID_PARAMS();
    error BTOKEN_MINT_DISALLOWED();

    /// @inheritdoc IBridgedERC20Initializable
    function init(
        address _owner,
        address _addressManager,
        address _srcToken,
        uint256 _srcChainId,
        uint8 _decimals,
        string calldata _symbol,
        string calldata _name
    )
        external
        initializer
    {
        // Check if provided parameters are valid
        LibBridgedToken.validateInputs(_srcToken, _srcChainId);
        __Essential_init(_owner, _addressManager);
        __ERC20_init(_name, _symbol);

        // Set contract properties
        srcToken = _srcToken;
        srcChainId = _srcChainId;
        __srcDecimals = _decimals;
    }

    /// @inheritdoc IBridgedERC20Migratable
    function changeMigrationStatus(
        address _migratingAddress,
        bool _migratingInbound
    )
        external
        whenNotPaused
        onlyFromNamed(LibStrings.B_ERC20_VAULT)
        nonReentrant
    {
        if (_migratingAddress == migratingAddress && _migratingInbound == migratingInbound) {
            revert BTOKEN_INVALID_PARAMS();
        }

        migratingAddress = _migratingAddress;
        migratingInbound = _migratingInbound;
        emit MigrationStatusChanged(_migratingAddress, _migratingInbound);
    }

    /// @inheritdoc IBridgedERC20
    function mint(address _account, uint256 _amount) external whenNotPaused nonReentrant {
        // mint is disabled while migrating outbound.
        if (isMigratingOut()) revert BTOKEN_MINT_DISALLOWED();

        address _migratingAddress = migratingAddress;
        if (msg.sender == _migratingAddress) {
            // Inbound migration
            emit MigratedFrom(_migratingAddress, _account, _amount);
        } else {
            // Bridging from vault
            _authorizedMintBurn(msg.sender);
        }

        _mint(_account, _amount);
    }

    /// @inheritdoc IBridgedERC20
    function burn(uint256 _amount) external whenNotPaused nonReentrant {
        if (isMigratingOut()) {
            // Outbound migration
            address _migratingAddress = migratingAddress;
            emit MigratedTo(_migratingAddress, msg.sender, _amount);
            // Ask the new bridged token to mint token for the user.
            IBridgedERC20(_migratingAddress).mint(msg.sender, _amount);
        } else {
            // When user wants to burn tokens only during 'migrating out' phase is possible. If
            // ERC20Vault burns the tokens, that will go through the burn(amount) function.
            _authorizedMintBurn(msg.sender);
        }

        _burn(msg.sender, _amount);
    }

    /// @inheritdoc IBridgedERC20
    function canonical() external view returns (address, uint256) {
        return (srcToken, srcChainId);
    }

    /// @notice Gets the number of decimal places of the token.
    /// @return The number of decimal places of the token.
    function decimals() public view override returns (uint8) {
        return __srcDecimals;
    }

    function isMigratingOut() public view returns (bool) {
        return migratingAddress != address(0) && !migratingInbound;
    }

    function supportsInterface(bytes4 _interfaceId) public pure returns (bool) {
        return _interfaceId == type(IBridgedERC20).interfaceId
            || _interfaceId == type(IBridgedERC20Initializable).interfaceId
            || _interfaceId == type(IBridgedERC20Migratable).interfaceId
            || _interfaceId == type(IERC20Upgradeable).interfaceId
            || _interfaceId == type(IERC20MetadataUpgradeable).interfaceId
            || _interfaceId == type(IERC165Upgradeable).interfaceId;
    }

    function _beforeTokenTransfer(
        address _from,
        address _to,
        uint256 _amount
    )
        internal
        override
        whenNotPaused
    {
        LibBridgedToken.checkToAddress(_to);
        return super._beforeTokenTransfer(_from, _to, _amount);
    }

    function _authorizedMintBurn(address addr)
        private
        onlyFromOwnerOrNamed(LibStrings.B_ERC20_VAULT)
    { }
}
