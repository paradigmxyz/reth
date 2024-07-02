// SPDX-License-Identifier: MIT
//  _____     _ _         _         _
// |_   _|_ _(_) |_____  | |   __ _| |__ ___
//   | |/ _` | | / / _ \ | |__/ _` | '_ (_-<
//   |_|\__,_|_|_\_\___/ |____\__,_|_.__/__/

pragma solidity ^0.8.20;

import "./TaikoL2.sol";

/// @title TaikoL2EIP1559Configurable
/// @notice Taiko L2 with a setter to change EIP-1559 configurations and states.
contract TaikoL2EIP1559Configurable is TaikoL2 {
    Config private _config;
    uint256[49] private __gap;

    event ConfigAndExcessChanged(Config config, uint64 gasExcess);

    error L2_INVALID_CONFIG();

    /// @notice Sets EIP1559 configuration and gas excess.
    /// @param config The new EIP1559 config.
    /// @param newGasExcess The new gas excess
    function setConfigAndExcess(
        Config memory config,
        uint64 newGasExcess
    )
        external
        virtual
        onlyOwner
    {
        if (config.gasTargetPerL1Block == 0) revert L2_INVALID_CONFIG();
        if (config.basefeeAdjustmentQuotient == 0) revert L2_INVALID_CONFIG();

        _config = config;
        gasExcess = newGasExcess;

        emit ConfigAndExcessChanged(config, newGasExcess);
    }

    function getConfig() public view override returns (Config memory) {
        return _config;
    }
}
