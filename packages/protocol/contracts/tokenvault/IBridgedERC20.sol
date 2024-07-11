// SPDX-License-Identifier: MIT
pragma solidity 0.8.24;

/// @title IBridgedERC20
/// @notice Interface for all bridged tokens.
/// @dev Here is the list of assumptions that guarantees that the bridged token can be bridged back
/// to it's canonical counterpart (by-default it is, but in case a third-party "native" token is set
/// and used in our bridge):
/// - The token should be ERC-20 compliant
/// - Supply increases should only be caused by mints from the vault. Notably, rebasing tokens are
/// not supported.
/// - Token balances should change by exactly the given amounts on `transfer`/`mint`/`burn`. Notable,
/// tokens with fees on transfers are not supported.
/// - If the bridged token is not directly deployed by the Bridge (ERC20Vault), - for example a USDT
/// token bytecode deployed on Taiko to support native tokens - it might be necessary to implement
/// an intermediary adapter contract which should conform mint() and burn() interfaces, so that the
/// ERC20Vault can call these actions on the adapter.
/// - If the bridged token is not directly deployed by the Bridge (ERC20Vault), but conforms the
/// mint() and burn() interface and the ERC20Vault has the right to perform these actions (has
/// minter/burner role).
/// - If the bridged token is directly deployed by our Bridge (ERC20Vault).
/// @custom:security-contact security@taiko.xyz
interface IBridgedERC20 {
    /// @notice Mints `amount` tokens and assigns them to the `account` address.
    /// @param _account The account to receive the minted tokens.
    /// @param _amount The amount of tokens to mint.
    function mint(address _account, uint256 _amount) external;

    /// @notice Burns tokens from msg.sender. This is only allowed if:
    /// - 1) tokens are migrating out to a new bridged token
    /// - 2) The token is burned by ERC20Vault to bridge back to the canonical chain.
    /// @param _amount The amount of tokens to burn.
    function burn(uint256 _amount) external;

    /// @notice Gets the canonical token's address and chain ID.
    /// @return The canonical token's address.
    /// @return The canonical token's chain ID.
    function canonical() external view returns (address, uint256);
}

/// @title IBridgedERC20Migratable
/// @custom:security-contact security@taiko.xyz
interface IBridgedERC20Migratable {
    /// @notice Starts or stops migration to/from a specified contract.
    /// @param _addr The address migrating 'to' or 'from'.
    /// @param _inbound If false then signals migrating 'from', true if migrating 'into'.
    function changeMigrationStatus(address _addr, bool _inbound) external;
}

/// @title IBridgedERC20Initializable
/// @custom:security-contact security@taiko.xyz
interface IBridgedERC20Initializable {
    /// @notice Initializes the contract.
    /// @param _owner The owner of this contract. msg.sender will be used if this value is zero.
    /// @param _addressManager The address of the {AddressManager} contract.
    /// @param _srcToken The source token address.
    /// @param _srcChainId The source chain ID.
    /// @param _decimals The number of decimal places of the source token.
    /// @param _symbol The symbol of the token.
    /// @param _name The name of the token.
    function init(
        address _owner,
        address _addressManager,
        address _srcToken,
        uint256 _srcChainId,
        uint8 _decimals,
        string calldata _symbol,
        string calldata _name
    )
        external;
}
