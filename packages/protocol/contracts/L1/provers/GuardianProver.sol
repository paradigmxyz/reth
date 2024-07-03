// SPDX-License-Identifier: MIT
//  _____     _ _         _         _
// |_   _|_ _(_) |_____  | |   __ _| |__ ___
//   | |/ _` | | / / _ \ | |__/ _` | '_ (_-<
//   |_|\__,_|_|_\_\___/ |____\__,_|_.__/__/

pragma solidity ^0.8.20;

import "./Guardians.sol";

/// @title GuardianProver
contract GuardianProver is Guardians {
    error PROVING_FAILED();

    /// @notice Initializes the contract with the provided address manager.
    /// @param _addressManager The address of the address manager contract.
    function init(address _addressManager) external initializer {
        __Essential_init(_addressManager);
    }

    /// @dev Called by guardians to approve a guardian proof
    function approve(
        TaikoData.BlockMetadata calldata meta,
        TaikoData.Transition calldata tran
    )
        external
        whenNotPaused
        nonReentrant
        returns (bool approved)
    {
        bytes32 hash = keccak256(abi.encode(meta, tran));
        approved = approve(meta.l2BlockNumber, hash);

        if (approved) {
            deleteApproval(hash);
            //ITaikoL1(resolve("taiko", false)).proveBlock(meta.id, abi.encode(meta, tran, proof));
        }
    }
}
