// SPDX-License-Identifier: MIT
pragma solidity 0.8.24;

import "@openzeppelin/contracts-upgradeable/token/ERC20/extensions/ERC20VotesUpgradeable.sol";
import "../common/EssentialContract.sol";
import "../common/LibStrings.sol";
import "../libs/LibAddress.sol";
import "../libs/LibMath.sol";
import "../signal/ISignalService.sol";
import "./IBridge.sol";
import "./IQuotaManager.sol";

/// @title Bridge
/// @notice See the documentation for {IBridge}.
/// @dev Labeled in AddressResolver as "bridge". Additionally, the code hash for the same address on
/// L1 and L2 may be different.
/// @custom:security-contact security@taiko.xyz
contract Bridge is EssentialContract, IBridge {
    using Address for address;
    using LibMath for uint256;
    using LibAddress for address;
    using LibAddress for address payable;

    struct ProcessingStats {
        uint32 gasUsedInFeeCalc;
        uint32 proofSize;
        uint32 numCacheOps;
        bool processedByRelayer;
    }

    /// @dev A debug event for fine-tuning gas related constants in the future.
    event MessageProcessed(bytes32 indexed msgHash, Message message, ProcessingStats stats);

    /// @dev The amount of gas that will be deducted from message.gasLimit before calculating the
    /// invocation gas limit. This value should be fine-tuned with production data.
    uint32 public constant GAS_RESERVE = 800_000;

    /// @dev The gas overhead for both receiving and invoking a message, as well as the proof
    /// calldata cost.
    /// This value should be fine-tuned with production data.
    uint32 public constant GAS_OVERHEAD = 120_000;

    ///@dev The max proof size for a message to be processable by a relayer.
    uint256 public constant RELAYER_MAX_PROOF_BYTES = 200_000;

    /// @dev The amount of gas not to charge fee per cache operation.
    uint256 private constant _GAS_REFUND_PER_CACHE_OPERATION = 20_000;

    /// @dev The slot in transient storage of the call context. This is the keccak256 hash
    /// of "bridge.ctx_slot"
    bytes32 private constant _CTX_SLOT =
        0xe4ece82196de19aabe639620d7f716c433d1348f96ce727c9989a982dbadc2b9;

    /// @dev Gas limit for sending Ether.
    // - EOA gas used is < 21000
    // - For Loopring smart wallet, gas used is about 23000
    // - For Argent smart wallet on Ethereum, gas used is about 24000
    // - For Gnosis Safe wallet, gas used is about 28000
    uint256 private constant _SEND_ETHER_GAS_LIMIT = 35_000;

    /// @dev Place holder value when not using transient storage
    uint256 private constant _PLACEHOLDER = type(uint256).max;

    /// @notice The next message ID.
    /// @dev Slot 1.
    uint64 private __reserved1;
    uint64 public nextMessageId;

    /// @notice Mapping to store the status of a message from its hash.
    /// @dev Slot 2.
    mapping(bytes32 msgHash => Status status) public messageStatus;

    /// @dev Slots 3 and 4
    Context private __ctx;

    /// @dev Slot 5.
    uint256 private __reserved2;

    /// @dev Slot 6.
    uint256 private __reserved3;

    uint256[44] private __gap;

    error B_INVALID_CHAINID();
    error B_INVALID_CONTEXT();
    error B_INVALID_FEE();
    error B_INVALID_GAS_LIMIT();
    error B_INVALID_STATUS();
    error B_INVALID_VALUE();
    error B_INSUFFICIENT_GAS();
    error B_MESSAGE_NOT_SENT();
    error B_OUT_OF_ETH_QUOTA();
    error B_PERMISSION_DENIED();
    error B_PROOF_TOO_LARGE();
    error B_RETRY_FAILED();
    error B_SIGNAL_NOT_RECEIVED();

    modifier sameChain(uint64 _chainId) {
        if (_chainId != block.chainid) revert B_INVALID_CHAINID();
        _;
    }

    modifier diffChain(uint64 _chainId) {
        if (_chainId == 0 || _chainId == block.chainid) revert B_INVALID_CHAINID();
        _;
    }

    /// @notice Initializes the contract.
    /// @param _owner The owner of this contract. msg.sender will be used if this value is zero.
    /// @param _addressManager The address of the {AddressManager} contract.
    function init(address _owner, address _addressManager) external initializer {
        __Essential_init(_owner, _addressManager);
    }

    function init2() external onlyOwner reinitializer(2) {
        // reset some previously used slots for future reuse
        __reserved1 = 0;
        __reserved2 = 0;
        __reserved3 = 0;
    }

    /// @notice Delegates a given token's voting power to the bridge itself.
    /// @param _anyToken Any token that supports delegation.
    function selfDelegate(address _anyToken) external nonZeroAddr(_anyToken) {
        ERC20VotesUpgradeable(_anyToken).delegate(address(this));
    }

    /// @inheritdoc IBridge
    function sendMessage(Message calldata _message)
        external
        payable
        override
        nonZeroAddr(_message.srcOwner)
        nonZeroAddr(_message.destOwner)
        diffChain(_message.destChainId)
        whenNotPaused
        nonReentrant
        returns (bytes32 msgHash_, Message memory message_)
    {
        if (_message.gasLimit == 0) {
            if (_message.fee != 0) revert B_INVALID_FEE();
        } else if (_invocationGasLimit(_message) == 0) {
            revert B_INVALID_GAS_LIMIT();
        }

        // Check if the destination chain is enabled.
        (bool destChainEnabled,) = isDestChainEnabled(_message.destChainId);

        // Verify destination chain.
        if (!destChainEnabled) revert B_INVALID_CHAINID();

        // Ensure the sent value matches the expected amount.
        if (_message.value + _message.fee != msg.value) revert B_INVALID_VALUE();

        message_ = _message;

        // Configure message details and send signal to indicate message sending.
        message_.id = nextMessageId++;
        message_.from = msg.sender;
        message_.srcChainId = uint64(block.chainid);

        msgHash_ = hashMessage(message_);

        emit MessageSent(msgHash_, message_);
        ISignalService(resolve(LibStrings.B_SIGNAL_SERVICE, false)).sendSignal(msgHash_);
    }

    /// @inheritdoc IBridge
    function recallMessage(
        Message calldata _message,
        bytes calldata _proof
    )
        external
        sameChain(_message.srcChainId)
        diffChain(_message.destChainId)
        whenNotPaused
        nonReentrant
    {
        bytes32 msgHash = hashMessage(_message);
        _checkStatus(msgHash, Status.NEW);

        address signalService = resolve(LibStrings.B_SIGNAL_SERVICE, false);

        if (!ISignalService(signalService).isSignalSent(address(this), msgHash)) {
            revert B_MESSAGE_NOT_SENT();
        }

        _proveSignalReceived(
            signalService, signalForFailedMessage(msgHash), _message.destChainId, _proof
        );

        _updateMessageStatus(msgHash, Status.RECALLED);
        if (!_consumeEtherQuota(_message.value)) revert B_OUT_OF_ETH_QUOTA();

        // Execute the recall logic based on the contract's support for the
        // IRecallableSender interface
        if (_message.from.supportsInterface(type(IRecallableSender).interfaceId)) {
            _storeContext(msgHash, address(this), _message.srcChainId);

            // Perform recall
            IRecallableSender(_message.from).onMessageRecalled{ value: _message.value }(
                _message, msgHash
            );

            // Must reset the context after the message call
            _resetContext();
        } else {
            _message.srcOwner.sendEtherAndVerify(_message.value, _SEND_ETHER_GAS_LIMIT);
        }
    }

    /// @inheritdoc IBridge
    /// @dev To ensure successful execution, we recommend this transaction's gas limit not to be
    /// smaller than:
    /// `(message.gasLimit - GAS_RESERVE) * 64 / 63 + GAS_RESERVE`,
    /// Or we can use a simplified rule: `tx.gaslimit = message.gaslimit * 102%`.
    function processMessage(
        Message calldata _message,
        bytes calldata _proof
    )
        external
        whenNotPaused
        nonReentrant
        returns (Status status_, StatusReason reason_)
    {
        uint256 gasStart = gasleft();

        // same as `sameChain(_message.destChainId)` but without stack-too-deep
        if (_message.destChainId != block.chainid) revert B_INVALID_CHAINID();

        // same as `diffChain(_message.srcChainId)` but without stack-too-deep
        if (_message.srcChainId == 0 || _message.srcChainId == block.chainid) {
            revert B_INVALID_CHAINID();
        }

        ProcessingStats memory stats;
        stats.processedByRelayer = msg.sender != _message.destOwner;

        // If the gas limit is set to zero, only the owner can process the message.
        if (stats.processedByRelayer) {
            if (_message.gasLimit == 0) revert B_PERMISSION_DENIED();
            if (_proof.length > RELAYER_MAX_PROOF_BYTES) revert B_PROOF_TOO_LARGE();
        }

        bytes32 msgHash = hashMessage(_message);
        _checkStatus(msgHash, Status.NEW);

        address signalService = resolve(LibStrings.B_SIGNAL_SERVICE, false);

        stats.proofSize = uint32(_proof.length);
        stats.numCacheOps =
            _proveSignalReceived(signalService, msgHash, _message.srcChainId, _proof);

        if (!_consumeEtherQuota(_message.value + _message.fee)) revert B_OUT_OF_ETH_QUOTA();

        uint256 refundAmount;
        if (_unableToInvokeMessageCall(_message, signalService)) {
            // Handle special addresses and message.data encoded function calldata that don't
            // require or cannot proceed with actual invocation and mark message as DONE
            refundAmount = _message.value;
            status_ = Status.DONE;
            reason_ = StatusReason.INVOCATION_PROHIBITED;
        } else {
            uint256 gasLimit = stats.processedByRelayer ? _invocationGasLimit(_message) : gasleft();

            if (_invokeMessageCall(_message, msgHash, gasLimit, stats.processedByRelayer)) {
                status_ = Status.DONE;
                reason_ = StatusReason.INVOCATION_OK;
            } else {
                status_ = Status.RETRIABLE;
                reason_ = StatusReason.INVOCATION_FAILED;
            }
        }

        if (_message.fee != 0) {
            refundAmount += _message.fee;

            if (stats.processedByRelayer && _message.gasLimit != 0) {
                unchecked {
                    // The relayer (=message processor) needs to get paid from the fee, and below it
                    // the calculation mechanism of that.
                    // The high level overview is: "gasCharged * block.basefee" with some caveat.
                    // Sometimes over or under estimated and it has different reasons:
                    // - a rational relayer shall simulate transactions off-chain so he/she would
                    // exactly know if the txn is profitable or not.
                    // - need to have a buffer/small revenue to the realyer since it consumes
                    // maintenance and infra costs to operate
                    uint256 refund = stats.numCacheOps * _GAS_REFUND_PER_CACHE_OPERATION;
                    // Taking into account the encoded message calldata cost, and can count with 16
                    // gas per bytes (vs. checking each and every byte if zero or non-zero)
                    stats.gasUsedInFeeCalc = uint32(
                        GAS_OVERHEAD + gasStart + _messageCalldataCost(_message.data.length)
                            - gasleft()
                    );

                    uint256 gasCharged = refund.max(stats.gasUsedInFeeCalc) - refund;
                    uint256 maxFee = gasCharged * _message.fee / _message.gasLimit;
                    uint256 baseFee = gasCharged * block.basefee;
                    uint256 fee =
                        (baseFee >= maxFee ? maxFee : (maxFee + baseFee) >> 1).min(_message.fee);

                    refundAmount -= fee;
                    msg.sender.sendEtherAndVerify(fee, _SEND_ETHER_GAS_LIMIT);
                }
            }
        }

        _message.destOwner.sendEtherAndVerify(refundAmount, _SEND_ETHER_GAS_LIMIT);

        _updateMessageStatus(msgHash, status_);
        emit MessageProcessed(msgHash, _message, stats);
    }

    /// @inheritdoc IBridge
    function retryMessage(
        Message calldata _message,
        bool _isLastAttempt
    )
        external
        sameChain(_message.destChainId)
        diffChain(_message.srcChainId)
        whenNotPaused
        nonReentrant
    {
        bytes32 msgHash = hashMessage(_message);
        _checkStatus(msgHash, Status.RETRIABLE);

        if (!_consumeEtherQuota(_message.value)) revert B_OUT_OF_ETH_QUOTA();

        bool succeeded;
        if (_unableToInvokeMessageCall(_message, resolve(LibStrings.B_SIGNAL_SERVICE, false))) {
            succeeded = _message.destOwner.sendEther(_message.value, _SEND_ETHER_GAS_LIMIT, "");
        } else {
            if ((_message.gasLimit == 0 || _isLastAttempt) && msg.sender != _message.destOwner) {
                revert B_PERMISSION_DENIED();
            }

            // Attempt to invoke the messageCall.
            succeeded = _invokeMessageCall(_message, msgHash, gasleft(), false);
        }

        if (succeeded) {
            _updateMessageStatus(msgHash, Status.DONE);
        } else if (_isLastAttempt) {
            _updateMessageStatus(msgHash, Status.FAILED);

            ISignalService(resolve(LibStrings.B_SIGNAL_SERVICE, false)).sendSignal(
                signalForFailedMessage(msgHash)
            );
        } else {
            revert B_RETRY_FAILED();
        }
    }

    /// @inheritdoc IBridge
    function failMessage(Message calldata _message)
        external
        sameChain(_message.destChainId)
        diffChain(_message.srcChainId)
        whenNotPaused
        nonReentrant
    {
        if (msg.sender != _message.destOwner) revert B_PERMISSION_DENIED();

        bytes32 msgHash = hashMessage(_message);
        _checkStatus(msgHash, Status.RETRIABLE);

        _updateMessageStatus(msgHash, Status.FAILED);
        ISignalService(resolve(LibStrings.B_SIGNAL_SERVICE, false)).sendSignal(
            signalForFailedMessage(msgHash)
        );
    }

    /// @inheritdoc IBridge
    function isMessageSent(Message calldata _message) external view returns (bool) {
        if (_message.srcChainId != block.chainid) return false;
        return ISignalService(resolve(LibStrings.B_SIGNAL_SERVICE, false)).isSignalSent({
            _app: address(this),
            _signal: hashMessage(_message)
        });
    }

    /// @notice Checks if a msgHash has failed on its destination chain.
    /// This is the 'readonly' version of proveMessageFailed.
    /// @param _message The message.
    /// @param _proof The merkle inclusion proof.
    /// @return true if the message has failed, false otherwise.
    function isMessageFailed(
        Message calldata _message,
        bytes calldata _proof
    )
        external
        view
        returns (bool)
    {
        if (_message.srcChainId != block.chainid) return false;

        return _isSignalReceived(
            resolve(LibStrings.B_SIGNAL_SERVICE, false),
            signalForFailedMessage(hashMessage(_message)),
            _message.destChainId,
            _proof
        );
    }

    /// @notice Checks if a msgHash has been received on its source chain.
    /// This is the 'readonly' version of proveMessageReceived.
    /// @param _message The message.
    /// @param _proof The merkle inclusion proof.
    /// @return true if the message has been received, false otherwise.
    function isMessageReceived(
        Message calldata _message,
        bytes calldata _proof
    )
        external
        view
        returns (bool)
    {
        if (_message.destChainId != block.chainid) return false;
        return _isSignalReceived(
            resolve(LibStrings.B_SIGNAL_SERVICE, false),
            hashMessage(_message),
            _message.srcChainId,
            _proof
        );
    }

    /// @notice Checks if the destination chain is enabled.
    /// @param _chainId The destination chain ID.
    /// @return enabled_ True if the destination chain is enabled.
    /// @return destBridge_ The bridge of the destination chain.
    function isDestChainEnabled(uint64 _chainId)
        public
        view
        returns (bool enabled_, address destBridge_)
    {
        destBridge_ = resolve(_chainId, LibStrings.B_BRIDGE, true);
        enabled_ = destBridge_ != address(0);
    }

    /// @notice Gets the current context.
    /// @inheritdoc IBridge
    function context() external view returns (Context memory ctx_) {
        ctx_ = _loadContext();
        if (ctx_.msgHash == 0 || ctx_.msgHash == bytes32(_PLACEHOLDER)) {
            revert B_INVALID_CONTEXT();
        }
    }

    /// @inheritdoc IBridge
    function hashMessage(Message memory _message) public pure returns (bytes32) {
        return keccak256(abi.encode("TAIKO_MESSAGE", _message));
    }

    /// @notice Returns a signal representing a failed/recalled message.
    /// @param _msgHash The message hash.
    /// @return The failed representation of it as bytes32.
    function signalForFailedMessage(bytes32 _msgHash) public pure returns (bytes32) {
        return _msgHash ^ bytes32(uint256(Status.FAILED));
    }

    /// @notice Returns the minimal gas limit required for sending a given message.
    /// @param dataLength The length of message.data.
    /// @return The minimal gas limit required for sending this message.
    function getMessageMinGasLimit(uint256 dataLength) public pure returns (uint32) {
        return _messageCalldataCost(dataLength) + GAS_RESERVE;
    }

    /// @notice Checks if the given address can pause and/or unpause the bridge.
    /// @dev Considering that the watchdog is a hot wallet, in case its private key is leaked, we
    /// only allow watchdog to pause the bridge, but does not allow it to unpause the bridge.
    function _authorizePause(address addr, bool toPause) internal view override {
        // Owner and chain_pauser can pause/unpause the bridge.
        if (addr == owner() || addr == resolve(LibStrings.B_CHAIN_WATCHDOG, true)) return;

        // bridge_watchdog can pause the bridge, but cannot unpause it.
        if (toPause && addr == resolve(LibStrings.B_BRIDGE_WATCHDOG, true)) return;

        revert RESOLVER_DENIED();
    }

    /// @notice Invokes a call message on the Bridge.
    /// @param _message The call message to be invoked.
    /// @param _msgHash The hash of the message.
    /// @param _shouldCheckForwardedGas True to check gasleft is sufficient for target function
    /// invocation.
    /// @return success_ A boolean value indicating whether the message call was successful.
    /// @dev This function updates the context in the state before and after the
    /// message call.
    function _invokeMessageCall(
        Message calldata _message,
        bytes32 _msgHash,
        uint256 _gasLimit,
        bool _shouldCheckForwardedGas
    )
        private
        returns (bool success_)
    {
        assert(_message.from != address(this));

        if (_gasLimit == 0) return false;

        _storeContext(_msgHash, _message.from, _message.srcChainId);

        address to = _message.to;
        uint256 value = _message.value;
        bytes memory data = _message.data;
        uint256 gasLeft;

        assembly {
            success_ := call(_gasLimit, to, value, add(data, 0x20), mload(data), 0, 0)
            gasLeft := gas()
        }

        if (_shouldCheckForwardedGas) {
            _checkForwardedGas(gasLeft, _gasLimit);
        }
        _resetContext();
    }

    /// @notice Updates the status of a bridge message.
    /// @dev If the new status is different from the current status in the
    /// mapping, the status is updated and an event is emitted.
    /// @param _msgHash The hash of the message.
    /// @param _status The new status of the message.
    function _updateMessageStatus(bytes32 _msgHash, Status _status) private {
        if (messageStatus[_msgHash] == _status) revert B_INVALID_STATUS();
        messageStatus[_msgHash] = _status;
        emit MessageStatusChanged(_msgHash, _status);
    }

    /// @notice Resets the call context
    function _resetContext() private {
        if (LibNetwork.isDencunSupported(block.chainid)) {
            _storeContext(bytes32(0), address(0), uint64(0));
        } else {
            _storeContext(
                bytes32(_PLACEHOLDER), address(uint160(_PLACEHOLDER)), uint64(_PLACEHOLDER)
            );
        }
    }

    /// @notice Stores the call context
    /// @param _msgHash The message hash.
    /// @param _from The sender's address.
    /// @param _srcChainId The source chain ID.
    function _storeContext(bytes32 _msgHash, address _from, uint64 _srcChainId) private {
        if (LibNetwork.isDencunSupported(block.chainid)) {
            assembly {
                tstore(_CTX_SLOT, _msgHash)
                tstore(add(_CTX_SLOT, 1), _from)
                tstore(add(_CTX_SLOT, 2), _srcChainId)
            }
        } else {
            __ctx = Context(_msgHash, _from, _srcChainId);
        }
    }

    /// @notice Checks if the signal was received and caches cross-chain data if requested.
    /// @param _signalService The signal service address.
    /// @param _signal The signal.
    /// @param _chainId The ID of the chain the signal is stored on.
    /// @param _proof The merkle inclusion proof.
    /// @return numCacheOps_ Num of cached items
    function _proveSignalReceived(
        address _signalService,
        bytes32 _signal,
        uint64 _chainId,
        bytes calldata _proof
    )
        private
        returns (uint32 numCacheOps_)
    {
        try ISignalService(_signalService).proveSignalReceived(
            _chainId, resolve(_chainId, LibStrings.B_BRIDGE, false), _signal, _proof
        ) returns (uint256 numCacheOps) {
            numCacheOps_ = uint32(numCacheOps);
        } catch {
            revert B_SIGNAL_NOT_RECEIVED();
        }
    }

    /// @notice Consumes a given amount of Ether from quota manager.
    /// @param _amount The amount of Ether to consume.
    /// @return true if quota manager has unlimited quota for Ether or the given amount of Ether is
    /// consumed already.
    function _consumeEtherQuota(uint256 _amount) private returns (bool) {
        address quotaManager = resolve(LibStrings.B_QUOTA_MANAGER, true);
        if (quotaManager == address(0)) return true;

        try IQuotaManager(quotaManager).consumeQuota(address(0), _amount) {
            return true;
        } catch {
            return false;
        }
    }

    /// @notice Loads and returns the call context.
    /// @return ctx_ The call context.
    function _loadContext() private view returns (Context memory) {
        if (LibNetwork.isDencunSupported(block.chainid)) {
            bytes32 msgHash;
            address from;
            uint64 srcChainId;
            assembly {
                msgHash := tload(_CTX_SLOT)
                from := tload(add(_CTX_SLOT, 1))
                srcChainId := tload(add(_CTX_SLOT, 2))
            }
            return Context(msgHash, from, srcChainId);
        } else {
            return __ctx;
        }
    }

    /// @notice Checks if the signal was received.
    /// This is the 'readonly' version of _proveSignalReceived.
    /// @param _signalService The signal service address.
    /// @param _signal The signal.
    /// @param _chainId The ID of the chain the signal is stored on.
    /// @param _proof The merkle inclusion proof.
    /// @return true if the message was received.
    function _isSignalReceived(
        address _signalService,
        bytes32 _signal,
        uint64 _chainId,
        bytes calldata _proof
    )
        private
        view
        returns (bool)
    {
        try ISignalService(_signalService).verifySignalReceived(
            _chainId, resolve(_chainId, LibStrings.B_BRIDGE, false), _signal, _proof
        ) {
            return true;
        } catch {
            return false;
        }
    }

    function _checkStatus(bytes32 _msgHash, Status _expectedStatus) private view {
        if (messageStatus[_msgHash] != _expectedStatus) revert B_INVALID_STATUS();
    }

    function _unableToInvokeMessageCall(
        Message calldata _message,
        address _signalService
    )
        private
        view
        returns (bool)
    {
        if (_message.to == address(0)) return true;
        if (_message.to == address(this)) return true;
        if (_message.to == _signalService) return true;

        return _message.data.length >= 4
            && bytes4(_message.data) != IMessageInvocable.onMessageInvocation.selector
            && _message.to.isContract();
    }

    function _invocationGasLimit(Message calldata _message) private pure returns (uint256) {
        uint256 minGasRequired = getMessageMinGasLimit(_message.data.length);
        unchecked {
            return minGasRequired.max(_message.gasLimit) - minGasRequired;
        }
    }

    function _messageCalldataCost(uint256 dataLength) private pure returns (uint32) {
        // The abi encoding of A = (Message calldata msg) is 10 * 32 bytes
        // + 32 bytes (A is a dynamic tuple, offset to first elements)
        // + 32 bytes (offset to last bytes element of Message)
        // + 32 bytes (padded encoding of length of Message.data + dataLength
        //   (padded to 32 // bytes) = 13 * 32 + ((dataLength + 31) / 32 * 32).
        // Non-zero calldata cost per byte is 16.
        unchecked {
            return uint32(((dataLength + 31) / 32 * 32 + 416) << 4);
        }
    }

    /// @dev Suggested by OpenZeppelin and copied from
    /// https://github.com/OpenZeppelin/openzeppelin-contracts/
    /// blob/83c7e45092dac350b070c421cd2bf7105616cf1a/contracts/
    /// metatx/ERC2771Forwarder.sol#L327C1-L370C6
    ///
    /// @dev Checks if the requested gas was correctly forwarded to the callee.
    /// As a consequence of https://eips.ethereum.org/EIPS/eip-150[EIP-150]:
    /// - At most `gasleft() - floor(gasleft() / 64)` is forwarded to the callee.
    /// - At least `floor(gasleft() / 64)` is kept in the caller.
    ///
    /// It reverts consuming all the available gas if the forwarded gas is not the requested gas.
    ///
    /// IMPORTANT: The `gasLeft` parameter should be measured exactly at the end of the forwarded
    /// call.
    /// Any gas consumed in between will make room for bypassing this check.
    function _checkForwardedGas(uint256 _gasLeft, uint256 _gasRequested) private pure {
        // To avoid insufficient gas griefing attacks, as referenced in
        // https://ronan.eth.limo/blog/ethereum-gas-dangers/
        //
        // A malicious relayer can attempt to shrink the gas forwarded so that the underlying call
        // reverts out-of-gas
        // but the forwarding itself still succeeds. In order to make sure that the subcall received
        // sufficient gas,
        // we will inspect gasleft() after the forwarding.
        //
        // Let X be the gas available before the subcall, such that the subcall gets at most X * 63
        // / 64.
        // We can't know X after CALL dynamic costs, but we want it to be such that X * 63 / 64 >=
        // req.gas.
        // Let Y be the gas used in the subcall. gasleft() measured immediately after the subcall
        // will be gasleft() = X - Y.
        // If the subcall ran out of gas, then Y = X * 63 / 64 and gasleft() = X - Y = X / 64.
        // Under this assumption req.gas / 63 > gasleft() is true is true if and only if
        // req.gas / 63 > X / 64, or equivalently req.gas > X * 63 / 64.
        // This means that if the subcall runs out of gas we are able to detect that insufficient
        // gas was passed.
        //
        // We will now also see that req.gas / 63 > gasleft() implies that req.gas >= X * 63 / 64.
        // The contract guarantees Y <= req.gas, thus gasleft() = X - Y >= X - req.gas.
        // -    req.gas / 63 > gasleft()
        // -    req.gas / 63 >= X - req.gas
        // -    req.gas >= X * 63 / 64
        // In other words if req.gas < X * 63 / 64 then req.gas / 63 <= gasleft(), thus if the
        // relayer behaves honestly
        // the forwarding does not revert.
        if (_gasLeft < _gasRequested / 63) {
            // We explicitly trigger invalid opcode to consume all gas and bubble-up the effects,
            // since
            // neither revert or assert consume all gas since Solidity 0.8.20
            // https://docs.soliditylang.org/en/v0.8.20/control-structures.html#panic-via-assert-and-error-via-require
            /// @solidity memory-safe-assembly
            assembly {
                invalid()
            }
        }
    }
}
