// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "./TaikoL1TestBase.sol";
/*
contract TaikoL1_NoCooldown is TaikoL1 {
    function getConfig() public view override returns (TaikoData.Config memory config) {
        config = TaikoL1.getConfig();
        // over-write the following
        config.maxBlocksToVerifyPerProposal = 0;
        config.blockMaxProposals = 10;
        config.blockRingBufferSize = 12;
        config.livenessBond = 1e18; // 1 Taiko token
    }
}

contract Verifier {
    fallback(bytes calldata) external returns (bytes memory) {
        return bytes.concat(keccak256("taiko"));
    }
}

contract TaikoL1Test is TaikoL1TestBase {
    function deployTaikoL1() internal override returns (TaikoL1) {
        return TaikoL1(
            payable(
                deployProxy({ name: "taiko", impl: address(new TaikoL1_NoCooldown()), data: "" })
            )
        );
    }

    /// @dev Test we can propose, prove, then verify more blocks than
    /// 'blockMaxProposals'
    function test_L1_more_blocks_than_ring_buffer_size() external {
        giveEthAndTko(Alice, 1e8 ether, 100 ether);
        // This is a very weird test (code?) issue here.
        // If this line (or Bob's query balance) is uncommented,
        // Alice/Bob has no balance.. (Causing reverts !!!)
        console2.log("Alice balance:", tko.balanceOf(Alice));
        giveEthAndTko(Bob, 1e8 ether, 100 ether);
        console2.log("Bob balance:", tko.balanceOf(Bob));
        giveEthAndTko(Carol, 1e8 ether, 100 ether);
        // Bob
        vm.prank(Bob, Bob);

        bytes32 parentHash = GENESIS_BLOCK_HASH;

        for (uint256 blockId = 1; blockId < conf.blockMaxProposals * 3; blockId++) {
            //printVariables("before propose");
            (TaikoData.BlockMetadata memory meta,) = proposeBlock(Alice, Bob, 1_000_000, 1024);
            //printVariables("after propose");
            mine(1);

            bytes32 blockHash = bytes32(1e10 + blockId);
            bytes32 signalRoot = bytes32(1e9 + blockId);
            proveBlock(Bob, Bob, meta, parentHash, blockHash, signalRoot, meta.minTier, "");
            vm.roll(block.number + 15 * 12);

            uint16 minTier = meta.minTier;
            vm.warp(block.timestamp + L1.getTier(minTier).cooldownWindow + 1);

            verifyBlock(Carol, 1);
            parentHash = blockHash;
        }
        printVariables("");
    }

    /// @dev Test more than one block can be proposed, proven, & verified in the
    ///      same L1 block.
    function test_L1_multiple_blocks_in_one_L1_block() external {
        giveEthAndTko(Alice, 1000 ether, 1000 ether);
        console2.log("Alice balance:", tko.balanceOf(Alice));
        giveEthAndTko(Bob, 1e8 ether, 100 ether);
        console2.log("Bob balance:", tko.balanceOf(Bob));
        giveEthAndTko(Carol, 1e8 ether, 100 ether);
        // Bob
        vm.prank(Bob, Bob);

        bytes32 parentHash = GENESIS_BLOCK_HASH;

        for (uint256 blockId = 1; blockId <= 20; ++blockId) {
            printVariables("before propose");
            (TaikoData.BlockMetadata memory meta,) = proposeBlock(Alice, Bob, 1_000_000, 1024);
            printVariables("after propose");

            bytes32 blockHash = bytes32(1e10 + blockId);
            bytes32 signalRoot = bytes32(1e9 + blockId);

            proveBlock(Bob, Bob, meta, parentHash, blockHash, signalRoot, meta.minTier, "");
            vm.roll(block.number + 15 * 12);
            uint16 minTier = meta.minTier;
            vm.warp(block.timestamp + L1.getTier(minTier).cooldownWindow + 1);

            verifyBlock(Alice, 2);
            parentHash = blockHash;
        }
        printVariables("");
    }

    /// @dev Test verifying multiple blocks in one transaction
    function test_L1_verifying_multiple_blocks_once() external {
        giveEthAndTko(Alice, 1000 ether, 1000 ether);
        console2.log("Alice balance:", tko.balanceOf(Alice));
        giveEthAndTko(Bob, 1e8 ether, 100 ether);
        console2.log("Bob balance:", tko.balanceOf(Bob));
        giveEthAndTko(Carol, 1e8 ether, 100 ether);
        // Bob
        vm.prank(Bob, Bob);

        bytes32 parentHash = GENESIS_BLOCK_HASH;

        for (uint256 blockId = 1; blockId <= conf.blockMaxProposals; blockId++) {
            printVariables("before propose");
            (TaikoData.BlockMetadata memory meta,) = proposeBlock(Alice, Bob, 1_000_000, 1024);
            printVariables("after propose");

            bytes32 blockHash = bytes32(1e10 + blockId);
            bytes32 signalRoot = bytes32(1e9 + blockId);

            proveBlock(Bob, Bob, meta, parentHash, blockHash, signalRoot, meta.minTier, "");
            parentHash = blockHash;
        }

        vm.roll(block.number + 15 * 12);
        verifyBlock(Alice, conf.blockMaxProposals - 1);
        printVariables("after verify");
        verifyBlock(Alice, conf.blockMaxProposals);
        printVariables("after verify");
    }

    /// @dev getCrossChainBlockHash tests
    function test_L1_getCrossChainBlockHash0() external {
        bytes32 genHash = L1.getSyncedSnippet(0).blockHash;
        assertEq(GENESIS_BLOCK_HASH, genHash);

        // Reverts if block is not yet verified!
        vm.expectRevert(TaikoErrors.L1_BLOCK_MISMATCH.selector);
        L1.getSyncedSnippet(1);
    }

    /// @dev getSyncedSnippet tests
    function test_L1_getSyncedSnippet() external {
        uint64 count = 10;
        // Declare here so that block prop/prove/verif. can be used in 1 place
        TaikoData.BlockMetadata memory meta;
        bytes32 blockHash;
        bytes32 signalRoot;
        bytes32[] memory parentHashes = new bytes32[](count);
        parentHashes[0] = GENESIS_BLOCK_HASH;

        giveEthAndTko(Alice, 1e6 ether, 100_000 ether);
        console2.log("Alice balance:", tko.balanceOf(Alice));
        giveEthAndTko(Bob, 1e7 ether, 100_000 ether);
        console2.log("Bob balance:", tko.balanceOf(Bob));

        // Propose blocks
        for (uint64 blockId = 1; blockId < count; ++blockId) {
            printVariables("before propose");
            (meta,) = proposeBlock(Alice, Bob, 1_000_000, 1024);
            mine(5);

            blockHash = bytes32(1e10 + uint256(blockId));
            signalRoot = bytes32(1e9 + uint256(blockId));

            proveBlock(
                Bob, Bob, meta, parentHashes[blockId - 1], blockHash, signalRoot, meta.minTier, ""
            );

            vm.roll(block.number + 15 * 12);
            uint16 minTier = meta.minTier;
            vm.warp(block.timestamp + L1.getTier(minTier).cooldownWindow + 1);

            verifyBlock(Carol, 1);

            // Querying written blockhash
            assertEq(L1.getSyncedSnippet(blockId).blockHash, blockHash);

            mine(5);
            parentHashes[blockId] = blockHash;
        }

        uint64 queriedBlockId = 1;
        bytes32 expectedSR = bytes32(1e9 + uint256(queriedBlockId));

        assertEq(expectedSR, L1.getSyncedSnippet(queriedBlockId).signalRoot);

        // 2nd
        queriedBlockId = 2;
        expectedSR = bytes32(1e9 + uint256(queriedBlockId));
        assertEq(expectedSR, L1.getSyncedSnippet(queriedBlockId).signalRoot);

        // Not found -> reverts
        vm.expectRevert(TaikoErrors.L1_BLOCK_MISMATCH.selector);
        L1.getSyncedSnippet((count + 1));
    }
}
*/