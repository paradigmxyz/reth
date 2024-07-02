// SPDX-License-Identifier: MIT
//  _____     _ _         _         _
// |_   _|_ _(_) |_____  | |   __ _| |__ ___
//   | |/ _` | | / / _ \ | |__/ _` | '_ (_-<
//   |_|\__,_|_|_\_\___/ |____\__,_|_.__/__/

pragma solidity ^0.8.20;

import "../common/EssentialContract.sol";
import "../common/ICrossChainSync.sol";
import "../libs/LibMath.sol";
import "../signal/ISignalService.sol";
import "./Lib1559Math.sol";
import "./TaikoL2Signer.sol";

/// @title TaikoL2
/// @notice Taiko L2 is a smart contract that handles cross-layer message
/// verification and manages EIP-1559 gas pricing for Layer 2 (L2) operations.
/// It is used to anchor the latest L1 block details to L2 for cross-layer
/// communication, manage EIP-1559 parameters for gas pricing, and store
/// verified L1 block information.
contract TaikoL2 is EssentialContract, TaikoL2Signer, ICrossChainSync {
    using LibMath for uint256;

    struct Config {
        uint32 gasTargetPerL1Block;
        uint8 basefeeAdjustmentQuotient;
    }

    // Mapping from L2 block numbers to their block hashes.
    // All L2 block hashes will be saved in this mapping.
    mapping(uint256 blockId => bytes32 blockHash) public l2Hashes;
    mapping(uint256 l1height => ICrossChainSync.Snippet) public snippets;

    // A hash to check the integrity of public inputs.
    address public signalService; // slot 3
    bytes32 public publicInputHash; // slot 4

    uint64 public gasExcess; // slot 5
    uint64 public latestSyncedL1Height;

    uint256[145] private __gap;

    event Anchored(bytes32 parentHash, uint64 gasExcess);

    error L2_BASEFEE_MISMATCH();
    error L2_INVALID_CHAIN_ID();
    error L2_INVALID_PARAM();
    error L2_INVALID_SENDER();
    error L2_PUBLIC_INPUT_HASH_MISMATCH();
    error L2_TOO_LATE();

    /// @notice Initializes the TaikoL2 contract.
    /// @param _signalService Address of the {ISignalService} contract.
    /// @param _gasExcess The initial gasExcess.
    function init(address _signalService, uint64 _gasExcess) external initializer {
        __Essential_init();

        if (_signalService == address(0)) revert L2_INVALID_PARAM();
        signalService = _signalService;

        if (block.chainid <= 1 || block.chainid >= type(uint64).max) {
            revert L2_INVALID_CHAIN_ID();
        }

        if (block.number == 0) {
            // This is the case in real L2 genesis
        } else if (block.number == 1) {
            // This is the case in tests
            uint256 parentHeight = block.number - 1;
            l2Hashes[parentHeight] = blockhash(parentHeight);
        } else {
            revert L2_TOO_LATE();
        }

        gasExcess = _gasExcess;
        (publicInputHash,) = _calcPublicInputHash(block.number);
    }

    /// @notice Anchors the latest L1 block details to L2 for cross-layer
    /// message verification.
    /// @param l1BlockHash The latest L1 block hash when this block was
    /// proposed.
    /// @param l1SignalRoot The latest value of the L1 signal root.
    /// @param l1Height The latest L1 block height when this block was proposed.
    /// @param parentGasUsed The gas used in the parent block.
    function anchor(
        bytes32 l1BlockHash,
        bytes32 l1SignalRoot,
        uint64 l1Height,
        uint32 parentGasUsed
    )
        external
    {
        if (
            l1BlockHash == 0 || l1SignalRoot == 0 || l1Height == 0
                || (block.number != 1 && parentGasUsed == 0)
        ) revert L2_INVALID_PARAM();

        if (msg.sender != GOLDEN_TOUCH_ADDRESS) revert L2_INVALID_SENDER();

        uint256 parentId;
        unchecked {
            parentId = block.number - 1;
        }

        // Verify ancestor hashes
        (bytes32 publicInputHashOld, bytes32 publicInputHashNew) = _calcPublicInputHash(parentId);
        if (publicInputHash != publicInputHashOld) {
            revert L2_PUBLIC_INPUT_HASH_MISMATCH();
        }

        Config memory config = getConfig();

        // Verify the base fee per gas is correct
        uint256 basefee;
        (basefee, gasExcess) = _calc1559BaseFee(config, l1Height, parentGasUsed);
        if (!skipFeeCheck() && block.basefee != basefee) {
            revert L2_BASEFEE_MISMATCH();
        }

        // Store the L1's signal root as a signal to the local signal service to
        // allow for multi-hop bridging.
        ISignalService(signalService).sendSignal(l1SignalRoot);
        emit CrossChainSynced(uint64(block.number), l1Height, l1BlockHash, l1SignalRoot);

        // Update state variables
        l2Hashes[parentId] = blockhash(parentId);
        snippets[l1Height] = ICrossChainSync.Snippet({
            remoteBlockId: l1Height,
            syncedInBlock: uint64(block.number),
            blockHash: l1BlockHash,
            signalRoot: l1SignalRoot
        });
        publicInputHash = publicInputHashNew;
        latestSyncedL1Height = l1Height;

        emit Anchored(blockhash(parentId), gasExcess);
    }

    /// @inheritdoc ICrossChainSync
    function getSyncedSnippet(uint64 blockId)
        public
        view
        override
        returns (ICrossChainSync.Snippet memory)
    {
        uint256 id = blockId == 0 ? latestSyncedL1Height : blockId;
        return snippets[id];
    }

    /// @notice Gets the basefee and gas excess using EIP-1559 configuration for
    /// the given parameters.
    /// @param l1Height The synced L1 height in the next Taiko block
    /// @param parentGasUsed Gas used in the parent block.
    /// @return basefee The calculated EIP-1559 base fee per gas.
    function getBasefee(
        uint64 l1Height,
        uint32 parentGasUsed
    )
        public
        view
        returns (uint256 basefee)
    {
        (basefee,) = _calc1559BaseFee(getConfig(), l1Height, parentGasUsed);
    }

    /// @notice Retrieves the block hash for the given L2 block number.
    /// @param blockId The L2 block number to retrieve the block hash for.
    /// @return The block hash for the specified L2 block id, or zero if the
    /// block id is greater than or equal to the current block number.
    function getBlockHash(uint64 blockId) public view returns (bytes32) {
        if (blockId >= block.number) return 0;
        if (blockId >= block.number - 256) return blockhash(blockId);
        return l2Hashes[blockId];
    }

    /// @notice Returns EIP1559 related configurations
    function getConfig() public view virtual returns (Config memory config) {
        // 4x Ethereum gas target, if we assume most of the time, L2 block time
        // is 3s, and each block is full (gasUsed is 15_000_000), then its
        // ~60_000_000, if the  network is congester than that, the base fee
        // will increase.
        config.gasTargetPerL1Block = 15 * 1e6 * 4;
        config.basefeeAdjustmentQuotient = 8;
    }

    /// @notice Tells if we need to validate basefee (for simulation).
    /// @return Returns true to skip checking basefee mismatch.
    function skipFeeCheck() public pure virtual returns (bool) { }

    function _calcPublicInputHash(uint256 blockId)
        private
        view
        returns (bytes32 publicInputHashOld, bytes32 publicInputHashNew)
    {
        bytes32[256] memory inputs;

        // Unchecked is safe because it cannot overflow.
        unchecked {
            // Put the previous 255 blockhashes (excluding the parent's) into a
            // ring buffer.
            for (uint256 i; i < 255 && blockId >= i + 1; ++i) {
                uint256 j = blockId - i - 1;
                inputs[j % 255] = blockhash(j);
            }
        }

        inputs[255] = bytes32(block.chainid);

        assembly {
            publicInputHashOld := keccak256(inputs, 8192 /*mul(256, 32)*/ )
        }

        inputs[blockId % 255] = blockhash(blockId);
        assembly {
            publicInputHashNew := keccak256(inputs, 8192 /*mul(256, 32)*/ )
        }
    }

    function _calc1559BaseFee(
        Config memory config,
        uint64 l1Height,
        uint32 parentGasUsed
    )
        private
        view
        returns (uint256 _basefee, uint64 _gasExcess)
    {
        // gasExcess being 0 indicate the dynamic 1559 base fee is disabled.
        if (gasExcess > 0) {
            // We always add the gas used by parent block to the gas excess
            // value as this has already happend
            uint256 excess = uint256(gasExcess) + parentGasUsed;

            // Calculate how much more gas to issue to offset gas excess.
            // after each L1 block time, config.gasTarget more gas is issued,
            // the gas excess will be reduced accordingly.
            // Note that when latestSyncedL1Height is zero, we skip this step.
            uint256 numL1Blocks;
            if (latestSyncedL1Height > 0 && l1Height > latestSyncedL1Height) {
                numL1Blocks = l1Height - latestSyncedL1Height;
            }

            if (numL1Blocks > 0) {
                uint256 issuance = numL1Blocks * config.gasTargetPerL1Block;
                excess = excess > issuance ? excess - issuance : 1;
            }

            _gasExcess = uint64(excess.min(type(uint64).max));

            // The base fee per gas used by this block is the spot price at the
            // bonding curve, regardless the actual amount of gas used by this
            // block, however, the this block's gas used will affect the next
            // block's base fee.
            _basefee = Lib1559Math.basefee(
                _gasExcess, uint256(config.basefeeAdjustmentQuotient) * config.gasTargetPerL1Block
            );
        }

        // Always make sure basefee is nonzero, this is required by the node.
        if (_basefee == 0) _basefee = 1;
    }
}
