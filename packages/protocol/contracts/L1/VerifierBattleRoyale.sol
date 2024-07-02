// SPDX-License-Identifier: MIT
//  _____     _ _         _         _
// |_   _|_ _(_) |_____  | |   __ _| |__ ___
//   | |/ _` | | / / _ \ | |__/ _` | '_ (_-<
//   |_|\__,_|_|_\_\___/ |____\__,_|_.__/__/

pragma solidity ^0.8.20;


import "../common/AddressResolver.sol";
import "../common/EssentialContract.sol";
import "../libs/LibAddress.sol";
import "./verifiers/IVerifier.sol";
import "./VerifierRegistry.sol";
import "./TaikoData.sol";

import "lib/openzeppelin-contracts/contracts/token/ERC20/IERC20.sol";

/// @title VerifierBattleRoyale
/// @notice A permissionless bounty to claim a reward for breaking a prover
contract VerifierBattleRoyale is EssentialContract {
    struct Bounty {
        uint startedAt;
        uint rate; // per second
        uint maxReward;
        uint claimedAt;
        address winner;
    }

    /// @dev Struct representing transition to be proven.
    struct ProofData {
        IVerifier verifier;
        TaikoData.Transition transition;
        bytes proof;
    }

    struct ProofBatch {
        TaikoData.BlockMetadata _block;
        ProofData[] proofs;
        address prover;
    }
    uint constant public PERCENTAGE_CLAIMED_IMMEDIATELY = 25;

    VerifierRegistry public verifierRegistry;
    mapping(address verifier => Bounty) public bounties;

    function init(address _addressManager) external initializer {
        __Essential_init(_addressManager);
    }

    /// @dev Proposes a Taiko L2 block.
    function openBounty(address verifier, Bounty memory bounty)
        external
        onlyOwner()
    {
        require(bounty.winner == address(0), "winner needs to be set to 0");
        bounties[verifier] = bounty;
    }

    // Allows anyone to claim the bounty be proving that some verifier is broken
    function claimBounty(address brokenVerifier, bytes calldata data)
        external
    {
        require(bounties[brokenVerifier].startedAt != 0, "bounty doesn't exist");
        require(bounties[brokenVerifier].winner == address(0), "bounty already claimed");

        // Decode the block data
        ProofBatch memory proofBatch = abi.decode(data, (ProofBatch));

        // Verify the all the proofs
        for (uint i = 0; i < proofBatch.proofs.length; i++) {
            IVerifier verifier = proofBatch.proofs[i].verifier;
            require(verifierRegistry.isVerifier(address(verifier)), "invalid verifier");
            verifier.verifyProof(proofBatch._block, proofBatch.proofs[i].transition, proofBatch.prover, proofBatch.proofs[i].proof);
        }

        if (proofBatch.proofs.length == 2) {
            /* Same verifier, same block, but different blockhashes/signalroots */

            require(proofBatch.proofs[0].verifier == proofBatch.proofs[1].verifier, "verifiers not the same");

            TaikoData.Transition memory transitionA = proofBatch.proofs[0].transition;
            TaikoData.Transition memory transitionB = proofBatch.proofs[1].transition;
            require(transitionA.parentHash == transitionB.parentHash, "parentHash not the same");
            require(
                transitionA.blockHash != transitionB.blockHash,
                "blockhash the same"
            );
        } else if (proofBatch.proofs.length == 3) {
            /* Multiple verifiers in a consensus show that another verifier is faulty */

            // Check that all verifiers are unique
            // Verify the proofs
            uint160 prevVerifier = 0;
            for (uint i = 0; i < proofBatch.proofs.length; i++) {
                require(prevVerifier >= uint160(address(proofBatch.proofs[i].verifier)), "duplicated verifier");
                prevVerifier = uint160(address(proofBatch.proofs[i].verifier));
            }

            // Reference proofs need to be placed first in the array, the faulty proof is listed last
            for (uint i = 0; i < proofBatch.proofs.length - 1; i++) {
                TaikoData.Transition memory transitionA = proofBatch.proofs[i].transition;
                TaikoData.Transition memory transitionB = proofBatch.proofs[i+1].transition;
                require(transitionA.parentHash == transitionB.parentHash, "parentHash not the same");
                if (i < proofBatch.proofs.length - 2) {
                    require(
                        transitionA.blockHash == transitionB.blockHash,
                        "blockhash the same"
                    );
                } else {
                    require(
                        transitionA.blockHash != transitionB.blockHash,
                        "blockhash the same"
                    );
                }
            }
        } else {
            revert("unsupported claim");
        }

        // Mark the bounty as claimed
        bounties[brokenVerifier].claimedAt = block.timestamp;
        bounties[brokenVerifier].winner = msg.sender;

        // Distribute part of the reward immediately
        uint initialReward = (calculateTotalReward(bounties[brokenVerifier]) * PERCENTAGE_CLAIMED_IMMEDIATELY) / 100;
        IERC20 tko = IERC20(resolve("taiko_token", false));
        tko.transfer(bounties[brokenVerifier].winner, initialReward);

        // Poison the verifier so it cannot be used anymore
        verifierRegistry.poisonVerifier(brokenVerifier);
    }

    // Called after the one who claimed a bounty has either disclosed
    // how the prover was broken or not
    function closeBounty(address verifier, bool disclosed)
        external
        onlyOwner()
    {
        require(bounties[verifier].winner != address(0), "bounty not claimed yet");

        // Transfer out the remaining locked part only the winner has disclosed how the prover was broken
        if (disclosed) {
            // Distribute the remaining part of the reward
            uint remainingReward = (calculateTotalReward(bounties[verifier]) * (100 - PERCENTAGE_CLAIMED_IMMEDIATELY)) / 100;
            IERC20 tko = IERC20(resolve("taiko_token", false));
            tko.transfer(bounties[verifier].winner, remainingReward);
        }

        // Delete the bounty
        // A new bounty needs to be started for the verifier
        delete bounties[verifier];
    }

    function calculateTotalReward(Bounty memory bounty)
        internal
        pure
        returns (uint)
    {
        uint accumulated = (bounty.claimedAt - bounty.startedAt) * bounty.rate;
        if (accumulated > bounty.maxReward) {
            accumulated = bounty.maxReward;
        }
        return accumulated;
    }
}
