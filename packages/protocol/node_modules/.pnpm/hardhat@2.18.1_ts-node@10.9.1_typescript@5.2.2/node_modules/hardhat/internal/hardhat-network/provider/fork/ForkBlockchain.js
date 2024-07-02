"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.ForkBlockchain = void 0;
const ethereumjs_block_1 = require("@nomicfoundation/ethereumjs-block");
const ethereumjs_util_1 = require("@nomicfoundation/ethereumjs-util");
const errors_1 = require("../../../core/providers/errors");
const BlockchainBase_1 = require("../BlockchainBase");
const output_1 = require("../output");
const ReadOnlyValidEIP2930Transaction_1 = require("../transactions/ReadOnlyValidEIP2930Transaction");
const ReadOnlyValidTransaction_1 = require("../transactions/ReadOnlyValidTransaction");
const ReadOnlyValidEIP1559Transaction_1 = require("../transactions/ReadOnlyValidEIP1559Transaction");
const ReadOnlyValidUnknownTypeTransaction_1 = require("../transactions/ReadOnlyValidUnknownTypeTransaction");
const rpcToBlockData_1 = require("./rpcToBlockData");
const rpcToTxData_1 = require("./rpcToTxData");
/* eslint-disable @nomicfoundation/hardhat-internal-rules/only-hardhat-error */
class ForkBlockchain extends BlockchainBase_1.BlockchainBase {
    constructor(_jsonRpcClient, _forkBlockNumber, common) {
        super(common);
        this._jsonRpcClient = _jsonRpcClient;
        this._forkBlockNumber = _forkBlockNumber;
        this._latestBlockNumber = this._forkBlockNumber;
    }
    getLatestBlockNumber() {
        return this._latestBlockNumber;
    }
    async getBlock(blockHashOrNumber) {
        if (typeof blockHashOrNumber === "bigint" &&
            this._data.isReservedBlock(blockHashOrNumber)) {
            this._data.fulfillBlockReservation(blockHashOrNumber);
        }
        let block;
        if (Buffer.isBuffer(blockHashOrNumber)) {
            block = await this._getBlockByHash(blockHashOrNumber);
            if (block === undefined) {
                throw new Error("Block not found");
            }
            return block;
        }
        block = await this._getBlockByNumber(BigInt(blockHashOrNumber));
        if (block === undefined) {
            throw new Error("Block not found");
        }
        return block;
    }
    async addBlock(block) {
        const blockNumber = BigInt(block.header.number);
        if (blockNumber !== this._latestBlockNumber + 1n) {
            throw new Error(`Invalid block number ${blockNumber}. Expected ${this._latestBlockNumber + 1n}`);
        }
        // When forking a network whose consensus is not the classic PoW,
        // we can't calculate the hash correctly.
        // Thus, we avoid this check for the first block after the fork.
        if (blockNumber > this._forkBlockNumber + 1n) {
            const parent = await this.getLatestBlock();
            if (!block.header.parentHash.equals(parent.hash())) {
                throw new Error("Invalid parent hash");
            }
        }
        this._latestBlockNumber++;
        const totalDifficulty = await this._computeTotalDifficulty(block);
        this._data.addBlock(block, totalDifficulty);
        return block;
    }
    reserveBlocks(count, interval, previousBlockStateRoot, previousBlockTotalDifficulty, previousBlockBaseFeePerGas) {
        super.reserveBlocks(count, interval, previousBlockStateRoot, previousBlockTotalDifficulty, previousBlockBaseFeePerGas);
        this._latestBlockNumber += count;
    }
    deleteLaterBlocks(block) {
        const blockNumber = block.header.number;
        const savedBlock = this._data.getBlockByNumber(blockNumber);
        if (savedBlock === undefined || !savedBlock.hash().equals(block.hash())) {
            throw new Error("Invalid block");
        }
        const nextBlockNumber = blockNumber + 1n;
        if (this._forkBlockNumber >= nextBlockNumber) {
            throw new Error("Cannot delete remote block");
        }
        this._delBlock(nextBlockNumber);
    }
    async getTotalDifficulty(blockHash) {
        let td = this._data.getTotalDifficulty(blockHash);
        if (td !== undefined) {
            return td;
        }
        // fetch block to check if it exists
        await this.getBlock(blockHash);
        td = this._data.getTotalDifficulty(blockHash);
        if (td === undefined) {
            throw new Error("This should never happen");
        }
        return td;
    }
    async getTransaction(transactionHash) {
        const tx = this.getLocalTransaction(transactionHash);
        if (tx === undefined) {
            const remote = await this._jsonRpcClient.getTransactionByHash(transactionHash);
            return this._processRemoteTransaction(remote);
        }
        return tx;
    }
    async getBlockByTransactionHash(transactionHash) {
        let block = this._data.getBlockByTransactionHash(transactionHash);
        if (block === undefined) {
            const remote = await this._jsonRpcClient.getTransactionByHash(transactionHash);
            this._processRemoteTransaction(remote);
            if (remote !== null && remote.blockHash !== null) {
                await this.getBlock(remote.blockHash);
                block = this._data.getBlockByTransactionHash(transactionHash);
            }
        }
        return block ?? null;
    }
    async getTransactionReceipt(transactionHash) {
        const local = this._data.getTransactionReceipt(transactionHash);
        if (local !== undefined) {
            return local;
        }
        const remote = await this._jsonRpcClient.getTransactionReceipt(transactionHash);
        if (remote !== null) {
            const receipt = await this._processRemoteReceipt(remote);
            return receipt ?? null;
        }
        return null;
    }
    getForkBlockNumber() {
        return this._forkBlockNumber;
    }
    async getLogs(filterParams) {
        if (filterParams.fromBlock <= this._forkBlockNumber) {
            let toBlock = filterParams.toBlock;
            let localLogs = [];
            if (toBlock > this._forkBlockNumber) {
                toBlock = this._forkBlockNumber;
                localLogs = this._data.getLogs({
                    ...filterParams,
                    fromBlock: this._forkBlockNumber + 1n,
                });
            }
            const remoteLogs = await this._jsonRpcClient.getLogs({
                fromBlock: filterParams.fromBlock,
                toBlock,
                address: filterParams.addresses.length === 1
                    ? filterParams.addresses[0]
                    : filterParams.addresses,
                topics: filterParams.normalizedTopics,
            });
            return remoteLogs.map(output_1.toRpcLogOutput).concat(localLogs);
        }
        return this._data.getLogs(filterParams);
    }
    async _getBlockByHash(blockHash) {
        const block = this._data.getBlockByHash(blockHash);
        if (block !== undefined) {
            return block;
        }
        const rpcBlock = await this._jsonRpcClient.getBlockByHash(blockHash, true);
        return this._processRemoteBlock(rpcBlock);
    }
    async _getBlockByNumber(blockNumber) {
        if (blockNumber > this._latestBlockNumber) {
            return undefined;
        }
        try {
            const block = await super.getBlock(blockNumber);
            return block;
        }
        catch { }
        const rpcBlock = await this._jsonRpcClient.getBlockByNumber(blockNumber, true);
        return this._processRemoteBlock(rpcBlock);
    }
    async _processRemoteBlock(rpcBlock) {
        if (rpcBlock === null ||
            rpcBlock.hash === null ||
            rpcBlock.number === null ||
            rpcBlock.number > this._forkBlockNumber) {
            return undefined;
        }
        const common = this._common.copy();
        // We set the common's hardfork depending on the remote block fields, to
        // prevent ethereumjs from throwing if unsupported fields are passed.
        // We use "berlin" for pre-EIP-1559 blocks (blocks without baseFeePerGas),
        // "merge" for blocks that have baseFeePerGas but not withdrawals,
        // and "shanghai" for blocks with withdrawals
        if (rpcBlock.baseFeePerGas === undefined) {
            common.setHardfork("berlin");
        }
        else if (rpcBlock.withdrawals === undefined) {
            common.setHardfork("merge");
        }
        else {
            common.setHardfork("shanghai");
        }
        // we don't include the transactions to add our own custom tx objects,
        // otherwise they are recreated with upstream classes
        const blockData = (0, rpcToBlockData_1.rpcToBlockData)({
            ...rpcBlock,
            transactions: [],
        });
        const block = ethereumjs_block_1.Block.fromBlockData(blockData, {
            common,
            // We use freeze false here because we add the transactions manually
            freeze: false,
            // don't validate things like the size of `extraData` in the header
            skipConsensusFormatValidation: true,
        });
        for (const transaction of rpcBlock.transactions) {
            let tx;
            if (transaction.type === undefined || transaction.type === 0n) {
                tx = new ReadOnlyValidTransaction_1.ReadOnlyValidTransaction(new ethereumjs_util_1.Address(transaction.from), (0, rpcToTxData_1.rpcToTxData)(transaction));
            }
            else if (transaction.type === 1n) {
                tx = new ReadOnlyValidEIP2930Transaction_1.ReadOnlyValidEIP2930Transaction(new ethereumjs_util_1.Address(transaction.from), (0, rpcToTxData_1.rpcToTxData)(transaction));
            }
            else if (transaction.type === 2n) {
                tx = new ReadOnlyValidEIP1559Transaction_1.ReadOnlyValidEIP1559Transaction(new ethereumjs_util_1.Address(transaction.from), (0, rpcToTxData_1.rpcToTxData)(transaction));
            }
            else {
                // we try to interpret unknown txs as legacy transactions, to support
                // networks like Arbitrum that have non-standards tx types
                try {
                    tx = new ReadOnlyValidUnknownTypeTransaction_1.ReadOnlyValidUnknownTypeTransaction(new ethereumjs_util_1.Address(transaction.from), Number(transaction.type), (0, rpcToTxData_1.rpcToTxData)(transaction));
                }
                catch (e) {
                    throw new errors_1.InternalError(`Could not process transaction with type ${transaction.type.toString()}`, e);
                }
            }
            block.transactions.push(tx);
        }
        this._data.addBlock(block, rpcBlock.totalDifficulty);
        return block;
    }
    _delBlock(blockNumber) {
        if (blockNumber <= this._forkBlockNumber) {
            throw new Error("Cannot delete remote block");
        }
        super._delBlock(blockNumber);
        this._latestBlockNumber = blockNumber - 1n;
    }
    _processRemoteTransaction(rpcTransaction) {
        if (rpcTransaction === null ||
            rpcTransaction.blockNumber === null ||
            rpcTransaction.blockNumber > this._forkBlockNumber) {
            return undefined;
        }
        let transaction;
        if (rpcTransaction.type === undefined || rpcTransaction.type === 0n) {
            transaction = new ReadOnlyValidTransaction_1.ReadOnlyValidTransaction(new ethereumjs_util_1.Address(rpcTransaction.from), (0, rpcToTxData_1.rpcToTxData)(rpcTransaction));
        }
        else if (rpcTransaction.type === 1n) {
            transaction = new ReadOnlyValidEIP2930Transaction_1.ReadOnlyValidEIP2930Transaction(new ethereumjs_util_1.Address(rpcTransaction.from), (0, rpcToTxData_1.rpcToTxData)(rpcTransaction));
        }
        else if (rpcTransaction.type === 2n) {
            transaction = new ReadOnlyValidEIP1559Transaction_1.ReadOnlyValidEIP1559Transaction(new ethereumjs_util_1.Address(rpcTransaction.from), (0, rpcToTxData_1.rpcToTxData)(rpcTransaction));
        }
        else {
            transaction = new ReadOnlyValidUnknownTypeTransaction_1.ReadOnlyValidUnknownTypeTransaction(new ethereumjs_util_1.Address(rpcTransaction.from), Number(rpcTransaction.type), (0, rpcToTxData_1.rpcToTxData)(rpcTransaction));
        }
        this._data.addTransaction(transaction);
        return transaction;
    }
    async _processRemoteReceipt(txReceipt) {
        if (txReceipt === null || txReceipt.blockNumber > this._forkBlockNumber) {
            return undefined;
        }
        const tx = await this.getTransaction(txReceipt.transactionHash);
        const receipt = (0, output_1.remoteReceiptToRpcReceiptOutput)(txReceipt, tx, (0, output_1.shouldShowTransactionTypeForHardfork)(this._common), (0, output_1.shouldShowEffectiveGasPriceForHardfork)(this._common));
        this._data.addTransactionReceipt(receipt);
        return receipt;
    }
}
exports.ForkBlockchain = ForkBlockchain;
//# sourceMappingURL=ForkBlockchain.js.map