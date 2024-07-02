"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.BlockchainData = void 0;
const ethereumjs_block_1 = require("@nomicfoundation/ethereumjs-block");
const ethereumjs_util_1 = require("@nomicfoundation/ethereumjs-util");
const ethereumjs_vm_1 = require("@nomicfoundation/ethereumjs-vm");
const errors_1 = require("../../core/errors");
const filter_1 = require("./filter");
class BlockchainData {
    constructor(_common) {
        this._common = _common;
        this._blocksByNumber = new Map();
        this._blocksByHash = new Map();
        this._blocksByTransactions = new Map();
        this._transactions = new Map();
        this._transactionReceipts = new Map();
        this._totalDifficulty = new Map();
        this._blockReservations = new Array();
    }
    reserveBlocks(first, count, interval, previousBlockStateRoot, previousBlockTotalDifficulty, previousBlockBaseFeePerGas) {
        const reservation = {
            first,
            last: first + count - 1n,
            interval,
            previousBlockStateRoot,
            previousBlockTotalDifficulty,
            previousBlockBaseFeePerGas,
        };
        this._blockReservations.push(reservation);
    }
    getBlockByNumber(blockNumber) {
        return this._blocksByNumber.get(blockNumber);
    }
    getBlockByHash(blockHash) {
        return this._blocksByHash.get((0, ethereumjs_util_1.bufferToHex)(blockHash));
    }
    getBlockByTransactionHash(transactionHash) {
        return this._blocksByTransactions.get((0, ethereumjs_util_1.bufferToHex)(transactionHash));
    }
    getTransaction(transactionHash) {
        return this._transactions.get((0, ethereumjs_util_1.bufferToHex)(transactionHash));
    }
    getTransactionReceipt(transactionHash) {
        return this._transactionReceipts.get((0, ethereumjs_util_1.bufferToHex)(transactionHash));
    }
    getTotalDifficulty(blockHash) {
        return this._totalDifficulty.get((0, ethereumjs_util_1.bufferToHex)(blockHash));
    }
    getLogs(filterParams) {
        const logs = [];
        for (let i = filterParams.fromBlock; i <= filterParams.toBlock; i++) {
            const block = this.getBlockByNumber(i);
            if (block === undefined ||
                !(0, filter_1.bloomFilter)(new ethereumjs_vm_1.Bloom(block.header.logsBloom), filterParams.addresses, filterParams.normalizedTopics)) {
                continue;
            }
            for (const transaction of block.transactions) {
                const receipt = this.getTransactionReceipt(transaction.hash());
                if (receipt !== undefined) {
                    logs.push(...(0, filter_1.filterLogs)(receipt.logs, {
                        fromBlock: filterParams.fromBlock,
                        toBlock: filterParams.toBlock,
                        addresses: filterParams.addresses,
                        normalizedTopics: filterParams.normalizedTopics,
                    }));
                }
            }
        }
        return logs;
    }
    addBlock(block, totalDifficulty) {
        const blockHash = (0, ethereumjs_util_1.bufferToHex)(block.hash());
        const blockNumber = block.header.number;
        this._blocksByNumber.set(blockNumber, block);
        this._blocksByHash.set(blockHash, block);
        this._totalDifficulty.set(blockHash, totalDifficulty);
        for (const transaction of block.transactions) {
            const transactionHash = (0, ethereumjs_util_1.bufferToHex)(transaction.hash());
            this._transactions.set(transactionHash, transaction);
            this._blocksByTransactions.set(transactionHash, block);
        }
    }
    /**
     * WARNING: this method can leave the blockchain in an invalid state where
     * there are gaps between blocks. Ideally we should have a method that removes
     * the given block and all the following blocks.
     */
    removeBlock(block) {
        const blockHash = (0, ethereumjs_util_1.bufferToHex)(block.hash());
        const blockNumber = block.header.number;
        this._blocksByNumber.delete(blockNumber);
        this._blocksByHash.delete(blockHash);
        this._totalDifficulty.delete(blockHash);
        for (const transaction of block.transactions) {
            const transactionHash = (0, ethereumjs_util_1.bufferToHex)(transaction.hash());
            this._transactions.delete(transactionHash);
            this._transactionReceipts.delete(transactionHash);
            this._blocksByTransactions.delete(transactionHash);
        }
    }
    addTransaction(transaction) {
        this._transactions.set((0, ethereumjs_util_1.bufferToHex)(transaction.hash()), transaction);
    }
    addTransactionReceipt(receipt) {
        this._transactionReceipts.set(receipt.transactionHash, receipt);
    }
    isReservedBlock(blockNumber) {
        return this._findBlockReservation(blockNumber) !== -1;
    }
    _findBlockReservation(blockNumber) {
        return this._blockReservations.findIndex((reservation) => reservation.first <= blockNumber && blockNumber <= reservation.last);
    }
    /**
     * WARNING: this method only removes the given reservation and can result in
     * gaps in the reservations array. Ideally we should have a method that
     * removes the given reservation and all the following reservations.
     */
    _removeReservation(index) {
        (0, errors_1.assertHardhatInvariant)(index in this._blockReservations, `Reservation ${index} does not exist`);
        const reservation = this._blockReservations[index];
        this._blockReservations.splice(index, 1);
        return reservation;
    }
    /**
     * Cancel and return the reservation that has block `blockNumber`
     */
    cancelReservationWithBlock(blockNumber) {
        return this._removeReservation(this._findBlockReservation(blockNumber));
    }
    fulfillBlockReservation(blockNumber) {
        // in addition to adding the given block, the reservation needs to be split
        // in two in order to accomodate access to the given block.
        const reservationIndex = this._findBlockReservation(blockNumber);
        (0, errors_1.assertHardhatInvariant)(reservationIndex !== -1, `No reservation to fill for block number ${blockNumber.toString()}`);
        // capture the timestamp before removing the reservation:
        const timestamp = this._calculateTimestampForReservedBlock(blockNumber);
        // split the block reservation:
        const oldReservation = this._removeReservation(reservationIndex);
        if (blockNumber !== oldReservation.first) {
            this._blockReservations.push({
                ...oldReservation,
                last: blockNumber - 1n,
            });
        }
        if (blockNumber !== oldReservation.last) {
            this._blockReservations.push({
                ...oldReservation,
                first: blockNumber + 1n,
            });
        }
        this.addBlock(ethereumjs_block_1.Block.fromBlockData({
            header: {
                number: blockNumber,
                stateRoot: oldReservation.previousBlockStateRoot,
                baseFeePerGas: oldReservation.previousBlockBaseFeePerGas,
                timestamp,
            },
        }, {
            common: this._common,
            skipConsensusFormatValidation: true,
        }), oldReservation.previousBlockTotalDifficulty);
    }
    _calculateTimestampForReservedBlock(blockNumber) {
        const reservationIndex = this._findBlockReservation(blockNumber);
        (0, errors_1.assertHardhatInvariant)(reservationIndex !== -1, `Block ${blockNumber.toString()} does not lie within any of the reservations.`);
        const reservation = this._blockReservations[reservationIndex];
        const blockNumberBeforeReservation = reservation.first - 1n;
        const blockBeforeReservation = this.getBlockByNumber(blockNumberBeforeReservation);
        (0, errors_1.assertHardhatInvariant)(blockBeforeReservation !== undefined, `Reservation after block ${blockNumberBeforeReservation.toString()} cannot be created because that block does not exist`);
        const previousTimestamp = this.isReservedBlock(blockNumberBeforeReservation)
            ? this._calculateTimestampForReservedBlock(blockNumberBeforeReservation)
            : blockBeforeReservation.header.timestamp;
        return (previousTimestamp +
            reservation.interval * (blockNumber - reservation.first + 1n));
    }
}
exports.BlockchainData = BlockchainData;
//# sourceMappingURL=BlockchainData.js.map