# Bridging contract flow

## High level overview

There are two parties at play which will interact with the `Bridge` contract, which is deployed on **both** the **source chain (srcChain)** and the **destination chain (destChain)**:

- The initiator of the bridge request (calls `Bridge.sendMessage`).
- The relayer (calls `Bridge.processMessage`).

The initiator will start the request, making it known on the Bridge contract via a signal. The relayer will pick this request up and process it.

## Diving deeper

Let's go deeper into the steps that occur when bridging ETH from srcChain to destChain:

### Send message / Send token

The bridge distinguishes 4 different token types: `Ether`, `ERC20`, `ERC1155`, `ERC721`. Ether is kept in the Bridge contract, and token vaults for ERC20, ERC1155, and ERC721 tokens must be deployed to the source and destination chain,

#### Bridging Ether

If user wants to bridge ether, he/she will initiate a bridge transaction with `sendMessage` on the source chain which includes:

```
    struct Message {
        // Message ID.
        uint256 id;
        // Message sender address.
        address from;
        // Source chain ID.
        uint64 srcChainId;
        // Destination chain ID where the `to` address lives.
        uint64 destChainId;
        // User address of the bridged asset.
        address user;
        // Destination user address.
        address to;
        // Alternate address to send any refund. If blank, defaults to user.
        address refundAddress;
        // value to invoke on the destination chain.
        uint256 value;
        // Processing fee for the relayer. Zero if user will process themselves.
        uint256 fee;
        // gasLimit to invoke on the destination chain.
        uint256 gasLimit;
        // callData to invoke on the destination chain.
        bytes data;
        // Optional memo.
        string memo;
    }
```

- `value` and `fee` must sum to `msg.value`.
- The destination chain's ID (must be enabled via setting `addressResolver` for `${chainID}.bridge`).

Inside the `sendMessage` call, the `msg.value` amount of Ether is kept in the Bridge contract, then a `signal` is created from the message, and a `key` is stored on the srcChain bridge contract address. The `key` is a hash of the `signal` and the srcChain bridge contract address. The `key` is stored on the `Bridge` contract with a value of `1`, and a `MessageSent` event is emitted for the relayer to pick up.

#### Bridging other tokens

If user wants to bridge other tokens (`ERC20`, `ERC1155` or `ERC721`.) he/she will just indirectly initiate a bridge transaction (`sendMessage`) by interacting with the corresponding token vault contracts.

In case of ERC20 the transaction can be initiated by initializing a struct (below) and calling `sendToken`:

```
    struct BridgeTransferOp {
        uint256 destChainId;
        address to;
        address token;
        uint256 amount;
        uint256 gasLimit;
        uint256 fee;
        address refundTo;
        string memo;
    }
```

In case of `ERC1155` or `ERC721`, the mechanism is the same but struct looks like this:

```
struct BridgeTransferOp {
        uint256 destChainId;
        address to;
        address token;
        uint256[] tokenIds;
        uint256[] amounts;
        uint256 gasLimit;
        uint256 fee;
        address refundTo;
        string memo;
    }
```

### Process message

If the `processingFee` is set to 0, only the user can call `processMessage`. Otherwise, either the user or an off-chain relayer can process the message. Let's explain the next steps in the case of a relayer -- the user will have to do the same steps anyways. In the case of a relayer, the relayer picks up the event and **generates a proof from srcChain** -- this can be obtained with `eth_getProof` on the srcChain bridge contract. This proof is sent along with the signal to `processMessage` on the destChain bridge contract.

The `processMessage` call will first check that the message has not been processed yet, this status is stored in the destination chain's bridge contract state as `statuses`. Next, the proof (that the message is indeed sent to the SignalService on the source chain) is checked inside `proveSignalReceived`. The proof demonstrates that the storage on the `Bridge` contract on srcChain contains the `key` with a value of `1`. `LibSecureMerkleTrie` takes the proof, the signal, and the message sender address to check the `key` is set on the srcChain bridge contract state. This verifies that the message is sent on srcChain. Next, `proveSignalReceived` gets the header hash on destChain of the header height specified in the proof. It then checks that this hash is equal to the hash specified in the proof. This will verify that the message is received on destChain.

The `processMessage` call will then proceed to invoke the message call, which will actually take the Ether from the vault and send it to the specified address. If it succeeds, it will mark the message as "DONE" on the srcChain bridge state. If it fails, it will mark the message as "RETRIABLE" and send the Ether back to the vault. Later, `retryMessage` can be called **only** by the user (`processMessage` cannot be called again for this message by the relayer).

Finally, any unused funds are sent back to the user as a refund.

### Failed bridging

If the `statuses` is "RETRIABLE" and - for whatever reason - the second try also cannot successfully initiate releasing the funds/tokens to the recipient on the destination chain, the `statuses` will be set to "FAILED". In this case the `recallMessage` shall be called on the source chain's Bridge contract (with `message` and `proof` input params), which will send the assets back to the user.
