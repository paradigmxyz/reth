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
        // meta.parentHash = GENESIS_BLOCK_HASH;
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
        bytes32 parentMetaHash;
        for (uint64 blockId = 1; blockId <= 1; blockId++) {
            printVariables("before propose");
            // Create metadata and propose the block
            meta = createBlockMetaData(parentMetaHash, Alice, blockId, 1, true);
            proposeBlock(Alice, Alice, meta);

            // Create proofs and prove a block
            BasedOperator.ProofBatch memory blockProofs = createProofs(meta, Alice, true);
            proveBlock(Alice, abi.encode(blockProofs));

            //Wait enought time and verify block
            vm.warp(uint32(block.timestamp + L1.SECURITY_DELAY_AFTER_PROVEN() + 1));
            vm.roll(block.number + 10);
            verifyBlock(1);
            printVariables("after verify");

            parentMetaHash = keccak256(abi.encode(meta));
        }
    }
}
