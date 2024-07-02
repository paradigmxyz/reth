"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.putGenesisBlock = void 0;
const ethereumjs_block_1 = require("@nomicfoundation/ethereumjs-block");
const ethereumjs_util_1 = require("@nomicfoundation/ethereumjs-util");
const date_1 = require("../../../util/date");
const hardforks_1 = require("../../../util/hardforks");
const getCurrentTimestamp_1 = require("./getCurrentTimestamp");
async function putGenesisBlock(blockchain, common, { initialDate, blockGasLimit: initialBlockGasLimit }, stateTrie, hardfork, initialMixHash, initialBaseFee) {
    const initialBlockTimestamp = initialDate !== undefined
        ? (0, date_1.dateToTimestampSeconds)(initialDate)
        : (0, getCurrentTimestamp_1.getCurrentTimestamp)();
    const isPostMerge = (0, hardforks_1.hardforkGte)(hardfork, hardforks_1.HardforkName.MERGE);
    const header = {
        timestamp: `0x${initialBlockTimestamp.toString(16)}`,
        gasLimit: initialBlockGasLimit,
        difficulty: isPostMerge ? 0 : 1,
        nonce: isPostMerge ? "0x0000000000000000" : "0x0000000000000042",
        extraData: "0x1234",
        stateRoot: (0, ethereumjs_util_1.bufferToHex)(stateTrie.root()),
    };
    if (isPostMerge) {
        header.mixHash = initialMixHash;
    }
    if (initialBaseFee !== undefined) {
        header.baseFeePerGas = initialBaseFee;
    }
    const genesisBlock = ethereumjs_block_1.Block.fromBlockData({
        header,
    }, {
        common,
        skipConsensusFormatValidation: true,
    });
    await blockchain.putBlock(genesisBlock);
}
exports.putGenesisBlock = putGenesisBlock;
//# sourceMappingURL=putGenesisBlock.js.map