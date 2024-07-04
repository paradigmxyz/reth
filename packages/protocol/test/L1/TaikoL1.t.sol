// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "./TaikoL1TestBase.sol";

contract TaikoL1Test is TaikoL1TestBase {
    function deployTaikoL1() internal override returns (TaikoL1) {
        return
            TaikoL1(payable(deployProxy({ name: "taiko", impl: address(new TaikoL1()), data: "" })));
    }

    function test_L1_proposeBlock() external {
        giveEthAndTko(Alice, 100 ether, 100 ether);

        TaikoData.BlockMetadata memory meta;

        vm.roll(block.number + 1);
        vm.warp(block.timestamp + 12);

        // console2.log(block.number);
        // meta.blockHash = randBytes32();
        // meta.parentMetaHash = GENESIS_BLOCK_HASH;
        // meta.l1Hash = blockhash(block.number - 1);
        // meta.difficulty = block.prevrandao;
        // meta.blobHash = randBytes32();
        // meta.coinbase = Alice;
        // meta.l2BlockNumber = 1;
        // meta.gasLimit = L1.getConfig().blockMaxGasLimit;
        // meta.l1StateBlockNumber = uint32(block.number-1);
        // meta.timestamp = uint64(block.timestamp - 12); // 1 block behind

        // meta.txListByteOffset = 0;
        // meta.txListByteSize = 0;
        // meta.blobUsed = true;

        for (uint64 blockId = 1; blockId <= 1; blockId++) {
            printVariables("before propose");
            meta = createBlockMetaData(Alice, blockId, 1, true);
            proposeBlock(Alice, Alice, meta);
            printVariables("after propose");

            BasedOperator.ProofBatch memory blockProofs = createProofs(meta, Alice, true);

            proveBlock(Alice, abi.encode(blockProofs));

            // bytes32 blockHash = bytes32(1e10 + blockId);
            // bytes32 stateRoot = bytes32(1e9 + blockId);

            // proveBlock(Alice, meta, parentHash, blockHash, stateRoot, meta.minTier, "");
            // parentHash = blockHash;
        }
    }
}
