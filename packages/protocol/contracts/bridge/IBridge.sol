// SPDX-License-Identifier: MIT
//  _____     _ _         _         _
// |_   _|_ _(_) |_____  | |   __ _| |__ ___
//   | |/ _` | | / / _ \ | |__/ _` | '_ (_-<
//   |_|\__,_|_|_\_\___/ |____\__,_|_.__/__/

pragma solidity ^0.8.20;

/// @title IBridge
/// @notice The bridge used in conjunction with the {ISignalService}.
/// @dev Ether is held by Bridges on L1 and L2s.
interface IBridge {
    struct Message {
        // Message ID.
        uint128 id;
        // The address, EOA or contract, that interacts with this bridge.
        address from;
        // Source chain ID.
        uint64 srcChainId;
        // Destination chain ID where the `to` address lives.
        uint64 destChainId;
        // The owner of the message.
        address owner;
        // The destination address on the destination chain.
        address to;
        // Alternate address to send any refund. If blank, defaults to owner.
        address refundTo;
        // value to invoke on the destination chain.
        uint256 value;
        // Processing fee for the relayer. Zero if owner will process themself.
        uint256 fee;
        // gasLimit to invoke on the destination chain.
        uint256 gasLimit;
        // callData to invoke on the destination chain.
        bytes data;
        // Optional memo.
        string memo;
    }

    // Struct representing the context of a bridge operation.
    struct Context {
        bytes32 msgHash; // Message hash.
        address from; // Sender's address.
        uint64 srcChainId; // Source chain ID.
    }

    /// @notice Sends a message to the destination chain and takes custody
    /// of Ether required in this contract. All extra Ether will be refunded.
    /// @param message The message to be sent.
    /// @return msgHash The hash of the sent message.
    /// @return updatedMessage The updated message sent.
    function sendMessage(Message calldata message)
        external
        payable
        returns (bytes32 msgHash, Message memory updatedMessage);

    /// @notice Returns the bridge state context.
    /// @return context The context of the current bridge operation.
    function context() external view returns (Context memory context);
}

/// @title IRecallableSender
/// @notice An interface that all recallable message senders shall implement.
interface IRecallableSender {
    function onMessageRecalled(
        IBridge.Message calldata message,
        bytes32 msgHash
    )
        external
        payable;
}
