// SPDX-License-Identifier: MIT
pragma solidity 0.8.24;

import "../common/EssentialContract.sol";
import "../common/LibStrings.sol";
import "../libs/LibTrieProof.sol";
import "./ISignalService.sol";

/// @title SignalService
/// @notice See the documentation in {ISignalService} for more details.
/// @dev Labeled in AddressResolver as "signal_service".
/// @custom:security-contact security@taiko.xyz
contract SignalService is EssentialContract, ISignalService {
    /// @notice Mapping to store the top blockId.
    /// @dev Slot 1.
    mapping(uint64 chainId => mapping(bytes32 kind => uint64 blockId)) public topBlockId;

    /// @notice Mapping to store the authorized addresses.
    /// @dev Slot 2.
    mapping(address addr => bool authorized) public isAuthorized;

    uint256[48] private __gap;

    struct CacheAction {
        bytes32 rootHash;
        bytes32 signalRoot;
        uint64 chainId;
        uint64 blockId;
        bool isFullProof;
        bool isLastHop;
        CacheOption option;
    }

    error SS_EMPTY_PROOF();
    error SS_INVALID_HOPS_WITH_LOOP();
    error SS_INVALID_LAST_HOP_CHAINID();
    error SS_INVALID_MID_HOP_CHAINID();
    error SS_INVALID_STATE();
    error SS_SIGNAL_NOT_FOUND();
    error SS_UNAUTHORIZED();

    /// @notice Initializes the contract.
    /// @param _owner The owner of this contract. msg.sender will be used if this value is zero.
    /// @param _addressManager The address of the {AddressManager} contract.
    function init(address _owner, address _addressManager) external initializer {
        __Essential_init(_owner, _addressManager);
    }

    /// @dev Authorize or deauthorize an address for calling syncChainData.
    /// @dev Note that addr is supposed to be TaikoL1 and TaikoL1 contracts deployed locally.
    /// @param _addr The address to be authorized or deauthorized.
    /// @param _authorize True if authorize, false otherwise.
    function authorize(address _addr, bool _authorize) external onlyOwner {
        if (isAuthorized[_addr] == _authorize) revert SS_INVALID_STATE();
        isAuthorized[_addr] = _authorize;
        emit Authorized(_addr, _authorize);
    }

    /// @inheritdoc ISignalService
    function sendSignal(bytes32 _signal) external returns (bytes32) {
        return _sendSignal(msg.sender, _signal, _signal);
    }

    /// @inheritdoc ISignalService
    function syncChainData(
        uint64 _chainId,
        bytes32 _kind,
        uint64 _blockId,
        bytes32 _chainData
    )
        external
        returns (bytes32)
    {
        if (!isAuthorized[msg.sender]) revert SS_UNAUTHORIZED();
        return _syncChainData(_chainId, _kind, _blockId, _chainData);
    }

    /// @inheritdoc ISignalService
    /// @dev This function may revert.
    function proveSignalReceived(
        uint64 _chainId,
        address _app,
        bytes32 _signal,
        bytes calldata _proof
    )
        external
        virtual
        whenNotPaused
        nonReentrant
        returns (uint256 numCacheOps_)
    {
        CacheAction[] memory actions = // actions for caching
         _verifySignalReceived(_chainId, _app, _signal, _proof, true);

        for (uint256 i; i < actions.length; ++i) {
            numCacheOps_ += _cache(actions[i]);
        }
    }

    /// @inheritdoc ISignalService
    /// @dev This function may revert.
    function verifySignalReceived(
        uint64 _chainId,
        address _app,
        bytes32 _signal,
        bytes calldata _proof
    )
        external
        view
    {
        _verifySignalReceived(_chainId, _app, _signal, _proof, false);
    }

    /// @inheritdoc ISignalService
    function isChainDataSynced(
        uint64 _chainId,
        bytes32 _kind,
        uint64 _blockId,
        bytes32 _chainData
    )
        public
        view
        nonZeroValue(_chainData)
        returns (bool)
    {
        bytes32 signal = signalForChainData(_chainId, _kind, _blockId);
        return _loadSignalValue(address(this), signal) == _chainData;
    }

    /// @inheritdoc ISignalService
    function isSignalSent(address _app, bytes32 _signal) public view returns (bool) {
        return _loadSignalValue(_app, _signal) != 0;
    }

    /// @inheritdoc ISignalService
    function getSyncedChainData(
        uint64 _chainId,
        bytes32 _kind,
        uint64 _blockId
    )
        public
        view
        returns (uint64 blockId_, bytes32 chainData_)
    {
        blockId_ = _blockId != 0 ? _blockId : topBlockId[_chainId][_kind];

        if (blockId_ != 0) {
            bytes32 signal = signalForChainData(_chainId, _kind, blockId_);
            chainData_ = _loadSignalValue(address(this), signal);
            if (chainData_ == 0) revert SS_SIGNAL_NOT_FOUND();
        }
    }

    /// @inheritdoc ISignalService
    function signalForChainData(
        uint64 _chainId,
        bytes32 _kind,
        uint64 _blockId
    )
        public
        pure
        returns (bytes32)
    {
        return keccak256(abi.encode(_chainId, _kind, _blockId));
    }

    /// @notice Returns the slot for a signal.
    /// @param _chainId The chainId of the signal.
    /// @param _app The address that initiated the signal.
    /// @param _signal The signal (message) that was sent.
    /// @return The slot for the signal.
    function getSignalSlot(
        uint64 _chainId,
        address _app,
        bytes32 _signal
    )
        public
        pure
        returns (bytes32)
    {
        return keccak256(abi.encodePacked("SIGNAL", _chainId, _app, _signal));
    }

    function _verifyHopProof(
        uint64 _chainId,
        address _app,
        bytes32 _signal,
        bytes32 _value,
        HopProof memory _hop,
        address _signalService
    )
        internal
        view
        virtual
        nonZeroAddr(_app)
        nonZeroValue(_signal)
        nonZeroValue(_value)
        returns (bytes32)
    {
        return LibTrieProof.verifyMerkleProof(
            _hop.rootHash,
            _signalService,
            getSignalSlot(_chainId, _app, _signal),
            _value,
            _hop.accountProof,
            _hop.storageProof
        );
    }

    function _authorizePause(address, bool) internal pure override notImplemented { }

    function _syncChainData(
        uint64 _chainId,
        bytes32 _kind,
        uint64 _blockId,
        bytes32 _chainData
    )
        private
        returns (bytes32 signal_)
    {
        signal_ = signalForChainData(_chainId, _kind, _blockId);
        _sendSignal(address(this), signal_, _chainData);

        if (topBlockId[_chainId][_kind] < _blockId) {
            topBlockId[_chainId][_kind] = _blockId;
        }
        emit ChainDataSynced(_chainId, _blockId, _kind, _chainData, signal_);
    }

    function _sendSignal(
        address _app,
        bytes32 _signal,
        bytes32 _value
    )
        private
        nonZeroAddr(_app)
        nonZeroValue(_signal)
        nonZeroValue(_value)
        returns (bytes32 slot_)
    {
        slot_ = getSignalSlot(uint64(block.chainid), _app, _signal);
        assembly {
            sstore(slot_, _value)
        }
        emit SignalSent(_app, _signal, slot_, _value);
    }

    function _cache(CacheAction memory _action) private returns (uint256 numCacheOps_) {
        // cache state root
        bool cacheStateRoot = _action.option == CacheOption.CACHE_BOTH
            || _action.option == CacheOption.CACHE_STATE_ROOT;

        if (cacheStateRoot && _action.isFullProof && !_action.isLastHop) {
            numCacheOps_ = 1;
            _syncChainData(
                _action.chainId, LibStrings.H_STATE_ROOT, _action.blockId, _action.rootHash
            );
        }

        // cache signal root
        bool cacheSignalRoot = _action.option == CacheOption.CACHE_BOTH
            || _action.option == CacheOption.CACHE_SIGNAL_ROOT;

        if (cacheSignalRoot && (_action.isFullProof || !_action.isLastHop)) {
            numCacheOps_ += 1;
            _syncChainData(
                _action.chainId, LibStrings.H_SIGNAL_ROOT, _action.blockId, _action.signalRoot
            );
        }
    }

    function _loadSignalValue(
        address _app,
        bytes32 _signal
    )
        private
        view
        nonZeroAddr(_app)
        nonZeroValue(_signal)
        returns (bytes32 value_)
    {
        bytes32 slot = getSignalSlot(uint64(block.chainid), _app, _signal);
        assembly {
            value_ := sload(slot)
        }
    }

    function _verifySignalReceived(
        uint64 _chainId,
        address _app,
        bytes32 _signal,
        bytes calldata _proof,
        bool _prepareCaching
    )
        private
        view
        nonZeroAddr(_app)
        nonZeroValue(_signal)
        returns (CacheAction[] memory actions)
    {
        HopProof[] memory hopProofs = abi.decode(_proof, (HopProof[]));
        if (hopProofs.length == 0) revert SS_EMPTY_PROOF();

        uint64[] memory trace = new uint64[](hopProofs.length - 1);

        if (_prepareCaching) {
            actions = new CacheAction[](hopProofs.length);
        }

        uint64 chainId = _chainId;
        address app = _app;
        bytes32 signal = _signal;
        bytes32 value = _signal;
        address signalService = resolve(chainId, LibStrings.B_SIGNAL_SERVICE, false);
        if (signalService == address(this)) revert SS_INVALID_MID_HOP_CHAINID();

        HopProof memory hop;
        bytes32 signalRoot;
        bool isFullProof;
        bool isLastHop;

        for (uint256 i; i < hopProofs.length; ++i) {
            hop = hopProofs[i];

            for (uint256 j; j < i; ++j) {
                if (trace[j] == hop.chainId) revert SS_INVALID_HOPS_WITH_LOOP();
            }

            signalRoot = _verifyHopProof(chainId, app, signal, value, hop, signalService);
            isLastHop = i == trace.length;
            if (isLastHop) {
                if (hop.chainId != block.chainid) revert SS_INVALID_LAST_HOP_CHAINID();
                signalService = address(this);
            } else {
                trace[i] = hop.chainId;

                if (hop.chainId == 0 || hop.chainId == block.chainid) {
                    revert SS_INVALID_MID_HOP_CHAINID();
                }
                signalService = resolve(hop.chainId, LibStrings.B_SIGNAL_SERVICE, false);
                if (signalService == address(this)) revert SS_INVALID_MID_HOP_CHAINID();
            }

            isFullProof = hop.accountProof.length != 0;

            if (_prepareCaching) {
                actions[i] = CacheAction(
                    hop.rootHash,
                    signalRoot,
                    chainId,
                    hop.blockId,
                    isFullProof,
                    isLastHop,
                    hop.cacheOption
                );
            }

            signal = signalForChainData(
                chainId,
                isFullProof ? LibStrings.H_STATE_ROOT : LibStrings.H_SIGNAL_ROOT,
                hop.blockId
            );
            value = hop.rootHash;
            chainId = hop.chainId;
            app = signalService;
        }

        if (value == 0 || value != _loadSignalValue(address(this), signal)) {
            revert SS_SIGNAL_NOT_FOUND();
        }
    }
}
