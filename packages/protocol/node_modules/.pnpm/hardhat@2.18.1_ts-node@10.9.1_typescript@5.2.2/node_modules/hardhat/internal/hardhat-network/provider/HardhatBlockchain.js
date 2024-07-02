"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.HardhatBlockchain = void 0;
const ethereumjs_util_1 = require("@nomicfoundation/ethereumjs-util");
const BlockchainBase_1 = require("./BlockchainBase");
/* eslint-disable @nomicfoundation/hardhat-internal-rules/only-hardhat-error */
class HardhatBlockchain extends BlockchainBase_1.BlockchainBase {
    constructor(common) {
        super(common);
        this._length = 0n;
    }
    getLatestBlockNumber() {
        return BigInt(this._length - 1n);
    }
    async addBlock(block) {
        this._validateBlock(block);
        const totalDifficulty = await this._computeTotalDifficulty(block);
        this._data.addBlock(block, totalDifficulty);
        this._length += 1n;
        return block;
    }
    reserveBlocks(count, interval, previousBlockStateRoot, previousBlockTotalDifficulty, previousBlockBaseFeePerGas) {
        super.reserveBlocks(count, interval, previousBlockStateRoot, previousBlockTotalDifficulty, previousBlockBaseFeePerGas);
        this._length += count;
    }
    deleteLaterBlocks(block) {
        const actual = this._data.getBlockByHash(block.hash());
        if (actual === undefined) {
            throw new Error("Invalid block");
        }
        this._delBlock(actual.header.number + 1n);
    }
    async getTotalDifficulty(blockHash) {
        const totalDifficulty = this._data.getTotalDifficulty(blockHash);
        if (totalDifficulty === undefined) {
            throw new Error("Block not found");
        }
        return totalDifficulty;
    }
    async getTransaction(transactionHash) {
        return this.getLocalTransaction(transactionHash);
    }
    async getBlockByTransactionHash(transactionHash) {
        const block = this._data.getBlockByTransactionHash(transactionHash);
        return block ?? null;
    }
    async getTransactionReceipt(transactionHash) {
        return this._data.getTransactionReceipt(transactionHash) ?? null;
    }
    async getLogs(filterParams) {
        return this._data.getLogs(filterParams);
    }
    _validateBlock(block) {
        const blockNumber = block.header.number;
        const parentHash = block.header.parentHash;
        const parent = this._data.getBlockByNumber(BigInt(blockNumber - 1n));
        if (BigInt(this._length) !== blockNumber) {
            throw new Error(`Invalid block number ${blockNumber}. Expected ${this._length}.`);
        }
        if ((blockNumber === 0n && !parentHash.equals((0, ethereumjs_util_1.zeros)(32))) ||
            (blockNumber > 0 &&
                parent !== undefined &&
                !parentHash.equals(parent.hash()))) {
            throw new Error("Invalid parent hash");
        }
    }
    _delBlock(blockNumber) {
        super._delBlock(blockNumber);
        this._length = blockNumber;
    }
}
exports.HardhatBlockchain = HardhatBlockchain;
//# sourceMappingURL=HardhatBlockchain.js.map