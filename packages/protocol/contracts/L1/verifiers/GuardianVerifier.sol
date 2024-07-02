// SPDX-License-Identifier: MIT
//  _____     _ _         _         _
// |_   _|_ _(_) |_____  | |   __ _| |__ ___
//   | |/ _` | | / / _ \ | |__/ _` | '_ (_-<
//   |_|\__,_|_|_\_\___/ |____\__,_|_.__/__/

pragma solidity ^0.8.20;

import "../../common/EssentialContract.sol";
import "../TaikoData.sol";
import "./IVerifier.sol";

/// @title GuardianVerifier
contract GuardianVerifier is EssentialContract, IVerifier {
    uint256[50] private __gap;

    error PERMISSION_DENIED();

    /// @notice Initializes the contract with the provided address manager.
    /// @param _addressManager The address of the address manager contract.
    function init(address _addressManager) external initializer {
        __Essential_init(_addressManager);
    }

    /// @inheritdoc IVerifier
    function verifyProof(
        TaikoData.BlockMetadata calldata /*_block*/,
        TaikoData.Transition calldata /*transition*/,
        address prover,
        bytes calldata /*proof*/
    )
        external
        view
    {
        if (prover != resolve("guardian_prover", false)) {
            revert PERMISSION_DENIED();
        }
    }
}
