"use strict";
var __createBinding = (this && this.__createBinding) || (Object.create ? (function(o, m, k, k2) {
    if (k2 === undefined) k2 = k;
    var desc = Object.getOwnPropertyDescriptor(m, k);
    if (!desc || ("get" in desc ? !m.__esModule : desc.writable || desc.configurable)) {
      desc = { enumerable: true, get: function() { return m[k]; } };
    }
    Object.defineProperty(o, k2, desc);
}) : (function(o, m, k, k2) {
    if (k2 === undefined) k2 = k;
    o[k2] = m[k];
}));
var __setModuleDefault = (this && this.__setModuleDefault) || (Object.create ? (function(o, v) {
    Object.defineProperty(o, "default", { enumerable: true, value: v });
}) : function(o, v) {
    o["default"] = v;
});
var __importStar = (this && this.__importStar) || function (mod) {
    if (mod && mod.__esModule) return mod;
    var result = {};
    if (mod != null) for (var k in mod) if (k !== "default" && Object.prototype.hasOwnProperty.call(mod, k)) __createBinding(result, mod, k);
    __setModuleDefault(result, mod);
    return result;
};
Object.defineProperty(exports, "__esModule", { value: true });
exports.BlockchainBase = void 0;
const ethereumjs_blockchain_1 = require("@nomicfoundation/ethereumjs-blockchain");
const ethereumjs_common_1 = require("@nomicfoundation/ethereumjs-common");
const errors_1 = require("../../core/errors");
const BigIntUtils = __importStar(require("../../util/bigint"));
const BlockchainData_1 = require("./BlockchainData");
/* eslint-disable @nomicfoundation/hardhat-internal-rules/only-hardhat-error */
class BlockchainBase {
    constructor(_common) {
        this._common = _common;
        this._data = new BlockchainData_1.BlockchainData(_common);
        // copied from blockchain.ts in @nomicfoundation/ethereumjs-blockchain
        switch (this._common.consensusAlgorithm()) {
            case ethereumjs_common_1.ConsensusAlgorithm.Casper:
                this.consensus = new ethereumjs_blockchain_1.CasperConsensus();
                break;
            case ethereumjs_common_1.ConsensusAlgorithm.Clique:
                this.consensus = new ethereumjs_blockchain_1.CliqueConsensus();
                break;
            case ethereumjs_common_1.ConsensusAlgorithm.Ethash:
                this.consensus = new ethereumjs_blockchain_1.EthashConsensus();
                break;
            default:
                throw new Error(`consensus algorithm ${this._common.consensusAlgorithm()} not supported`);
        }
    }
    addTransactionReceipts(receipts) {
        for (const receipt of receipts) {
            this._data.addTransactionReceipt(receipt);
        }
    }
    async delBlock(blockHash) {
        this.deleteBlock(blockHash);
    }
    deleteBlock(blockHash) {
        const block = this._data.getBlockByHash(blockHash);
        if (block === undefined) {
            throw new Error("Block not found");
        }
        this._delBlock(block.header.number);
    }
    async getBlock(blockHashOrNumber) {
        if ((typeof blockHashOrNumber === "number" ||
            BigIntUtils.isBigInt(blockHashOrNumber)) &&
            this._data.isReservedBlock(BigInt(blockHashOrNumber))) {
            this._data.fulfillBlockReservation(BigInt(blockHashOrNumber));
        }
        if (typeof blockHashOrNumber === "number") {
            const blockByNumber = this._data.getBlockByNumber(BigInt(blockHashOrNumber));
            if (blockByNumber === undefined) {
                throw new Error("Block not found");
            }
            return blockByNumber;
        }
        if (BigIntUtils.isBigInt(blockHashOrNumber)) {
            const blockByNumber = this._data.getBlockByNumber(blockHashOrNumber);
            if (blockByNumber === undefined) {
                throw new Error("Block not found");
            }
            return blockByNumber;
        }
        const blockByHash = this._data.getBlockByHash(blockHashOrNumber);
        if (blockByHash === undefined) {
            throw new Error("Block not found");
        }
        return blockByHash;
    }
    async getLatestBlock() {
        return this.getBlock(this.getLatestBlockNumber());
    }
    getLocalTransaction(transactionHash) {
        return this._data.getTransaction(transactionHash);
    }
    iterator(_name, _onBlock) {
        throw new Error("Method not implemented.");
    }
    async putBlock(block) {
        await this.addBlock(block);
    }
    reserveBlocks(count, interval, previousBlockStateRoot, previousBlockTotalDifficulty, previousBlockBaseFeePerGas) {
        this._data.reserveBlocks(this.getLatestBlockNumber() + 1n, count, interval, previousBlockStateRoot, previousBlockTotalDifficulty, previousBlockBaseFeePerGas);
    }
    copy() {
        throw new Error("Method not implemented.");
    }
    validateHeader(_header, _height) {
        throw new Error("Method not implemented.");
    }
    _delBlock(blockNumber) {
        let i = blockNumber;
        while (i <= this.getLatestBlockNumber()) {
            if (this._data.isReservedBlock(i)) {
                const reservation = this._data.cancelReservationWithBlock(i);
                i = reservation.last + 1n;
            }
            else {
                const current = this._data.getBlockByNumber(i);
                if (current !== undefined) {
                    this._data.removeBlock(current);
                }
                i++;
            }
        }
    }
    async _computeTotalDifficulty(block) {
        const difficulty = block.header.difficulty;
        const blockNumber = block.header.number;
        if (blockNumber === 0n) {
            return difficulty;
        }
        const parentBlock = await this.getBlock(blockNumber - 1n);
        const parentHash = parentBlock.hash();
        const parentTD = this._data.getTotalDifficulty(parentHash);
        (0, errors_1.assertHardhatInvariant)(parentTD !== undefined, "Parent block should have total difficulty");
        return parentTD + difficulty;
    }
}
exports.BlockchainBase = BlockchainBase;
//# sourceMappingURL=BlockchainBase.js.map