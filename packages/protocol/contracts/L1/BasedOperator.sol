// SPDX-License-Identifier: MIT
//  _____     _ _         _         _
// |_   _|_ _(_) |_____  | |   __ _| |__ ___
//   | |/ _` | | / / _ \ | |__/ _` | '_ (_-<
//   |_|\__,_|_|_\_\___/ |____\__,_|_.__/__/

pragma solidity ^0.8.20;

import "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import "../common/AddressResolver.sol";
import "../common/EssentialContract.sol";
import "../libs/LibAddress.sol";
import "./preconfs/ISequencerRegistry.sol";
import "./TaikoL1.sol";
import "./TaikoData.sol";
import "./TaikoErrors.sol";
import "./VerifierRegistry.sol";
import "./verifiers/IVerifier.sol";

/// @title BasedOperator
/// @notice A based operator for Taiko.
contract BasedOperator is EssentialContract, TaikoErrors {
    using LibAddress for address;

    struct Block {
        address assignedProver;
        uint96 bond;
    }

    /// @dev Struct representing transition to be proven.
    struct BlockProof {
        address prover;
    }

    /// @dev Struct representing transition to be proven.
    struct ProofData {
        IVerifier verifier;
        bytes proof;
    }

    /// @dev Struct representing transition to be proven.
    struct ProofBatch {
        TaikoData.BlockMetadata blockMetadata;
        TaikoData.Transition transition;
        ProofData[] proofs;
        address prover;
    }

    uint256 public constant PROVER_BOND = 1 ether / 10;
    uint256 public constant MAX_GAS_PROVER_PAYMENT = 50_000;
    uint256 public constant MAX_BLOCKS_TO_VERIFY = 5;
    uint256 public constant PROVING_WINDOW = 1 hours;

    address public treasury; // (?)

    mapping(uint256 => Block) public blocks;

    function init(address _owner, address _addressManager) external initializer {
        if (_addressManager == address(0)) {
            revert L1_INVALID_ADDRESS();
        }
        __Essential_init(_owner, _addressManager);
    }

    /// @dev Proposes a Taiko L2 block.
    function proposeBlock(
        bytes[] calldata data,
        bytes[] calldata txLists,
        address prover
    )
        external
        payable
        nonReentrant
        whenNotPaused
        returns (TaikoData.BlockMetadata[] memory _blocks)
    {
        if(txLists.length != 0) {
            require(data.length == txLists.length, "mismatched params length");
        }

        require(msg.value == PROVER_BOND, "Prover bond not expected");

        _blocks = new TaikoData.BlockMetadata[](data.length);
        for (uint i = 0; i < data.length; i++) {
            if(txLists.length != 0) {
                // If calldata, then pass forward the calldata
                _blocks[i] =  TaikoL1(resolve("taiko", false)).proposeBlock(data[i], txLists[i]);
            }
            else {
                // Blob otherwise
                _blocks[i] =  TaikoL1(resolve("taiko", false)).proposeBlock(data[i], "");
            }

            // Check if we have whitelisted proposers
            if (!_isProposerPermitted(_blocks[i])) {
                revert L1_INVALID_PROPOSER();
            }

            // Store who paid for proving the block
            blocks[_blocks[i].l2BlockNumber] = Block({ assignedProver: prover, bond: uint96(PROVER_BOND) });
        }

        // Verify some blocks
        _verifyBlocks(MAX_BLOCKS_TO_VERIFY);
    }

    /// @dev Proposes a Taiko L2 block.
    function proveBlock(bytes calldata data) external nonReentrant whenNotPaused {
        // Decode the block data
        ProofBatch memory proofBatch = abi.decode(data, (ProofBatch));

        // Check who can prove the block
        TaikoData.Block memory taikoBlock =
            TaikoL1(resolve("taiko", false)).getBlock(proofBatch.blockMetadata.l2BlockNumber);
        if (block.timestamp < taikoBlock.timestamp + PROVING_WINDOW) {
            require(
                proofBatch.prover == blocks[proofBatch.blockMetadata.l2BlockNumber].assignedProver,
                "assigned prover not the prover"
            );
        }

        VerifierRegistry verifierRegistry = VerifierRegistry(resolve("verifier_registry", false));
        TaikoL1 taiko = TaikoL1(resolve("taiko", false));
        // Verify the proofs
        uint160 prevVerifier = uint160(0);
        for (uint256 i = 0; i < proofBatch.proofs.length; i++) {
            IVerifier verifier = proofBatch.proofs[i].verifier;
            // Make sure each verifier is unique
            if (prevVerifier >= uint160(address(verifier))) {
                revert L1_INVALID_OR_DUPLICATE_VERIFIER();
            }
            // Make sure it's a valid verifier
            require(verifierRegistry.isVerifier(address(verifier)), "invalid verifier");
            // Verify the proof
            verifier.verifyProof(
                proofBatch.transition,
                keccak256(abi.encode(proofBatch.blockMetadata)),
                proofBatch.prover,
                proofBatch.proofs[i].proof
            );
            prevVerifier = uint160(address(verifier));
        }

        // Make sure the supplied proofs are sufficient.
        // Can use some custom logic here. but let's keep it simple
        require(proofBatch.proofs.length >= 3, "insufficient number of proofs");

        // Only allow an already proven block to be overwritten when the verifiers used are now
        // invalid
        // Get the currently stored transition
        TaikoData.TransitionState memory storedTransition = taiko.getTransition(
            proofBatch.blockMetadata.l2BlockNumber, proofBatch.transition.parentBlockHash
        );

        // Somehow we need to check if this is proven already and IF YES and transition is trying to
        // prove the same, then revert with "block already proven".
        if (
            storedTransition.isProven == true
                && storedTransition.blockHash == proofBatch.transition.blockHash
        ) {
            revert("block already proven");
        } else {
            // TODO(Brecht): Check that one of the verifiers is now poissoned
        }

        // Prove the block
        taiko.proveBlock(proofBatch.blockMetadata, proofBatch.transition, proofBatch.prover);

        // Verify some blocks
        _verifyBlocks(MAX_BLOCKS_TO_VERIFY);
    }

    function verifyBlocks(uint256 maxBlocksToVerify) external nonReentrant whenNotPaused {
        _verifyBlocks(maxBlocksToVerify);
    }

    function _verifyBlocks(uint256 maxBlocksToVerify) internal {
        TaikoL1 taiko = TaikoL1(resolve("taiko", false));
        uint256 lastVerifiedBlockIdBefore = taiko.getLastVerifiedBlockId();
        // Verify the blocks
        taiko.verifyBlocks(maxBlocksToVerify);
        uint256 lastVerifiedBlockIdAfter = taiko.getLastVerifiedBlockId();

        // So some additional checks on top of the standard checks done in the rollup contract
        for (
            uint256 blockId = lastVerifiedBlockIdBefore + 1;
            blockId <= lastVerifiedBlockIdAfter;
            blockId++
        ) {
            Block storage blk = blocks[blockId];

            // TODO(Brecht): Verify that all the verifers used to prove the block are still valid

            // Find out who the prover is
            TaikoData.Block memory previousBlock = taiko.getBlock(uint64(blockId) - 1);
            address prover = taiko.getTransition(uint64(blockId), previousBlock.blockHash).prover;

            // Return the bond or reward the other prover half the bond
            uint256 bondToReturn = blk.bond;
            if (prover != blk.assignedProver) {
                bondToReturn >>= 1;
                treasury.sendEtherAndVerify(bondToReturn, MAX_GAS_PROVER_PAYMENT);
            }
            prover.sendEtherAndVerify(bondToReturn, MAX_GAS_PROVER_PAYMENT);
        }
    }

    // Additinal proposer rules
    function _isProposerPermitted(TaikoData.BlockMetadata memory _block) private returns (bool) {
        if (_block.l2BlockNumber == 1) {
            // Only proposer_one can propose the first block after genesis
            address proposerOne = resolve("proposer_one", true);
            if (proposerOne != address(0) && msg.sender != proposerOne) {
                return false;
            }
        }

        // If there's a sequencer registry, check if the block can be proposed by the current
        // proposer
        ISequencerRegistry sequencerRegistry =
            ISequencerRegistry(resolve("sequencer_registry", true));
        if (sequencerRegistry != ISequencerRegistry(address(0))) {
            if (!sequencerRegistry.isEligibleSigner(msg.sender)) {
                return false;
            }
        }
        return true;
    }
}
