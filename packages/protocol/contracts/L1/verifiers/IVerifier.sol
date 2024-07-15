// SPDX-License-Identifier: MIT
//  _____     _ _         _         _
// |_   _|_ _(_) |_____  | |   __ _| |__ ___
//   | |/ _` | | / / _ \ | |__/ _` | '_ (_-<
//   |_|\__,_|_|_\_\___/ |____\__,_|_.__/__/

pragma solidity ^0.8.20;

import "../TaikoData.sol";

/// @title IVerifier Interface
/// @notice Defines the function that handles proof verification.
interface IVerifier {
    // Todo(Brecht/Dani):
    // This interface differs from taiko-mono's latest verifyProof(), mainly because we dont have
    // contestation for example, so no need to have TierProof structure. But further bundling the
    // structs into 1, and incorporate to TaikoData might be desireable, depending on how we need to
    // use.
    // See the taiko-mono used interface below this function signature.
    function verifyProof(
        TaikoData.Transition calldata transition,
        bytes32 blockMetaHash, //We dont need to post the full BlockMetadata struct
        address prover,
        bytes calldata proof
    )
        external;

    // As a reference, used by taiko-mono currently:
    // struct Context {
    //     bytes32 metaHash;
    //     bytes32 blobHash;
    //     address prover;
    //     uint64 blockId;
    //     bool isContesting;
    //     bool blobUsed;
    //     address msgSender;
    // }

    // /// @notice Verifies a proof.
    // /// @param _ctx The context of the proof verification.
    // /// @param _tran The transition to verify.
    // /// @param _proof The proof to verify.
    // function verifyProof(
    //     Context calldata _ctx,
    //     TaikoData.Transition calldata _tran,
    //     TaikoData.TierProof calldata _proof
    // )
    //     external;
}
