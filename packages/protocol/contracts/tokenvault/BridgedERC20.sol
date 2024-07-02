// SPDX-License-Identifier: MIT
//  _____     _ _         _         _
// |_   _|_ _(_) |_____  | |   __ _| |__ ___
//   | |/ _` | | / / _ \ | |__/ _` | '_ (_-<
//   |_|\__,_|_|_\_\___/ |____\__,_|_.__/__/

pragma solidity ^0.8.20;

import
    "lib/openzeppelin-contracts-upgradeable/contracts/token/ERC20/extensions/IERC20MetadataUpgradeable.sol";
import "lib/openzeppelin-contracts/contracts/utils/Strings.sol";

import "./LibBridgedToken.sol";
import "./BridgedERC20Base.sol";

/// @title BridgedERC20
/// @notice An upgradeable ERC20 contract that represents tokens bridged from
/// another chain.
contract BridgedERC20 is BridgedERC20Base, IERC20MetadataUpgradeable, ERC20Upgradeable {
    address public srcToken; // slot 1
    uint8 private srcDecimals;
    uint256 public srcChainId; // slot 2

    uint256[48] private __gap;

    error BTOKEN_CANNOT_RECEIVE();
    error BTOKEN_INVALID_PARAMS();

    /// @notice Initializes the contract.
    /// @dev Different BridgedERC20 Contract is deployed per unique _srcToken
    /// (e.g., one for USDC, one for USDT, etc.).
    /// @param _addressManager The address manager.
    /// @param _srcToken The source token address.
    /// @param _srcChainId The source chain ID.
    /// @param _decimals The number of decimal places of the source token.
    /// @param _symbol The symbol of the token.
    /// @param _name The name of the token.
    function init(
        address _addressManager,
        address _srcToken,
        uint256 _srcChainId,
        uint8 _decimals,
        string memory _symbol,
        string memory _name
    )
        external
        initializer
    {
        // Check if provided parameters are valid
        if (
            _srcToken == address(0) || _srcChainId == 0 || _srcChainId == block.chainid
                || bytes(_symbol).length == 0 || bytes(_name).length == 0
        ) {
            revert BTOKEN_INVALID_PARAMS();
        }

        // Initialize OwnerUUPSUpgradable and ERC20Upgradeable
        __Essential_init(_addressManager);
        __ERC20_init({ name_: _name, symbol_: _symbol });

        // Set contract properties
        srcToken = _srcToken;
        srcChainId = _srcChainId;
        srcDecimals = _decimals;
    }

    /// @notice Transfers tokens from the caller to another account.
    /// @dev Any address can call this. Caller must have at least 'amount' to
    /// call this.
    /// @param to The account to transfer tokens to.
    /// @param amount The amount of tokens to transfer.
    function transfer(
        address to,
        uint256 amount
    )
        public
        override(ERC20Upgradeable, IERC20Upgradeable)
        nonReentrant
        whenNotPaused
        returns (bool)
    {
        if (to == address(this)) revert BTOKEN_CANNOT_RECEIVE();
        return ERC20Upgradeable.transfer(to, amount);
    }

    /// @notice Transfers tokens from one account to another account.
    /// @dev Any address can call this. Caller must have allowance of at least
    /// 'amount' for 'from's tokens.
    /// @param from The account to transfer tokens from.
    /// @param to The account to transfer tokens to.
    /// @param amount The amount of tokens to transfer.
    function transferFrom(
        address from,
        address to,
        uint256 amount
    )
        public
        override(ERC20Upgradeable, IERC20Upgradeable)
        nonReentrant
        whenNotPaused
        returns (bool)
    {
        if (to == address(this)) {
            revert BTOKEN_CANNOT_RECEIVE();
        }
        return ERC20Upgradeable.transferFrom(from, to, amount);
    }

    /// @notice Gets the name of the token.
    /// @return The name.
    function name()
        public
        view
        override(ERC20Upgradeable, IERC20MetadataUpgradeable)
        returns (string memory)
    {
        return LibBridgedToken.buildName(super.name(), srcChainId);
    }

    /// @notice Gets the symbol of the bridged token.
    /// @return The symbol.
    function symbol()
        public
        view
        override(ERC20Upgradeable, IERC20MetadataUpgradeable)
        returns (string memory)
    {
        return LibBridgedToken.buildSymbol(super.symbol());
    }

    /// @notice Gets the number of decimal places of the token.
    /// @return The number of decimal places of the token.
    function decimals()
        public
        view
        override(ERC20Upgradeable, IERC20MetadataUpgradeable)
        returns (uint8)
    {
        return srcDecimals;
    }

    /// @notice Gets the canonical token's address and chain ID.
    /// @return The canonical token's address and chain ID.
    function canonical() public view returns (address, uint256) {
        return (srcToken, srcChainId);
    }

    function _mintToken(address account, uint256 amount) internal override {
        _mint(account, amount);
    }

    function _burnToken(address from, uint256 amount) internal override {
        _burn(from, amount);
    }
}
