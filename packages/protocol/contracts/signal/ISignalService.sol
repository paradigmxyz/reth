// SPDX-License-Identifier: MIT

//  _____     _ _         _         _
// |_   _|_ _(_) |_____  | |   __ _| |__ ___
//   | |/ _` | | / / _ \ | |__/ _` | '_ (_-<
//   |_|\__,_|_|_\_\___/ |____\__,_|_.__/__/

pragma solidity ^0.8.20;

/// @title ISignalService
/// @notice The SignalService contract serves as a secure cross-chain message
/// passing system. It defines methods for sending and verifying signals with
/// merkle proofs. The trust assumption is that the target chain has secure
/// access to the merkle root (such as Taiko injects it in the anchor
/// transaction). With this, verifying a signal is reduced to simply verifying
/// a merkle proof.

interface ISignalService {
    /// @notice Send a signal (message) by setting the storage slot to a value
    /// of 1.
    /// @param signal The signal (message) to send.
    /// @return storageSlot The location in storage where this signal is stored.
    function sendSignal(bytes32 signal) external returns (bytes32 storageSlot);

    /// @notice Verifies if a particular signal has already been sent.
    /// @param app The address that initiated the signal.
    /// @param signal The signal (message) to send.
    /// @return True if the signal has been sent, otherwise false.
    function isSignalSent(address app, bytes32 signal) external view returns (bool);

    /// @notice Verifies if a signal has been received on the target chain.
    /// @param srcChainId The identifier for the source chain from which the
    /// signal originated.
    /// @param app The address that initiated the signal.
    /// @param signal The signal (message) to send.
    /// @param proof Merkle proof that the signal was persisted on the
    /// source chain.
    /// @return True if the signal has been received, otherwise false.
    function proveSignalReceived(
        uint64 srcChainId,
        address app,
        bytes32 signal,
        bytes calldata proof
    )
        external
        view
        returns (bool);
}
