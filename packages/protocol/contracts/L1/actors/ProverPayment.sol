// SPDX-License-Identifier: MIT
//  _____     _ _         _         _
// |_   _|_ _(_) |_____  | |   __ _| |__ ___
//   | |/ _` | | / / _ \ | |__/ _` | '_ (_-<
//   |_|\__,_|_|_\_\___/ |____\__,_|_.__/__/

pragma solidity ^0.8.20;

import "../../common/AddressResolver.sol";
import "../../libs/LibAddress.sol";
import "../TaikoData.sol";
import "../BasedOperator.sol";

/// @title ProverPayment
/// @notice A library for handling block proposals in the Taiko protocol.
contract ProverPayment {
    using LibAddress for address;

    struct ProverAssignment {
        address prover;
        uint256 fee;
        uint64 maxBlockId;
        uint64 maxProposedIn;
        bytes32 metaHash;
        bytes signature;
    }

    BasedOperator public operator;

    mapping(address => uint256) public balances;

    // Max gas paying the prover. This should be large enough to prevent the
    // worst cases, usually block proposer shall be aware the risks and only
    // choose provers that cannot consume too much gas when receiving Ether.
    uint256 public constant MAX_GAS_PAYING_PROVER = 200_000;

    /// @dev Proposes a Taiko L2 block.
    function proposeBlock(
        bytes[] calldata data,
        bytes[] calldata txLists,
        bytes calldata proverAssignment
    )
        external
        payable
        returns (TaikoData.BlockMetadata[] memory _blocks)
    {
        // Decode the assignment data
        ProverAssignment memory assignment = abi.decode(proverAssignment, (ProverAssignment));

        // Subtract prover bond from the prover
        balances[assignment.prover] -= operator.PROVER_BOND();

        // Propose the block
        _blocks = operator.proposeBlock{ value: operator.PROVER_BOND() }(
            data, txLists, assignment.prover
        );

        uint64 highestl2BlockNumber = _blocks[_blocks.length-1].l2BlockNumber;

        // Hash the assignment with the blobHash, this hash will be signed by
        // the prover, therefore, we add a string as a prefix.
        // IMPORTANT!! Assignment now multi-block assignment!!
        bytes32 hash = hashAssignment(assignment);
        require(assignment.prover.isValidSignature(hash, assignment.signature), "invalid signature");

        // Check assignment validity
        require(
            (assignment.metaHash != 0 || keccak256(abi.encode(_blocks)) != assignment.metaHash)
                && (assignment.maxBlockId != 0 || highestl2BlockNumber > assignment.maxBlockId)
                && (assignment.maxProposedIn != 0 || block.number > assignment.maxProposedIn),
            "unexpected block"
        );

        // Pay the prover
        assignment.prover.sendEtherAndVerify(msg.value, MAX_GAS_PAYING_PROVER);
    }

    function hashAssignment(ProverAssignment memory assignment) internal view returns (bytes32) {
        return keccak256(
            abi.encode(
                "PROVER_ASSIGNMENT",
                address(this),
                block.chainid,
                assignment.metaHash,
                msg.value,
                assignment.maxBlockId,
                assignment.maxProposedIn
            )
        );
    }

    function deposit(address to) external payable {
        balances[to] += msg.value;
    }

    // TODO(Brecht): delay
    function witdraw(address from, address to, uint256 amount) external {
        balances[from] -= amount;
        to.sendEtherAndVerify(amount);
    }
}
