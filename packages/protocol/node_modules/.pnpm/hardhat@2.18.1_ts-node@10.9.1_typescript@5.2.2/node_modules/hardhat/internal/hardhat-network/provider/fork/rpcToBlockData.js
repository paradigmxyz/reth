"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.rpcToBlockData = void 0;
const rpcToTxData_1 = require("./rpcToTxData");
function rpcToBlockData(rpcBlock) {
    return {
        header: {
            parentHash: rpcBlock.parentHash,
            uncleHash: rpcBlock.sha3Uncles,
            coinbase: rpcBlock.miner,
            stateRoot: rpcBlock.stateRoot,
            transactionsTrie: rpcBlock.transactionsRoot,
            receiptTrie: rpcBlock.receiptsRoot,
            logsBloom: rpcBlock.logsBloom,
            difficulty: rpcBlock.difficulty,
            number: rpcBlock.number ?? undefined,
            gasLimit: rpcBlock.gasLimit,
            gasUsed: rpcBlock.gasUsed,
            timestamp: rpcBlock.timestamp,
            extraData: rpcBlock.extraData,
            mixHash: rpcBlock.mixHash,
            nonce: rpcBlock.nonce,
            baseFeePerGas: rpcBlock.baseFeePerGas,
            withdrawalsRoot: rpcBlock.withdrawalsRoot,
        },
        transactions: rpcBlock.transactions.map(rpcToTxData_1.rpcToTxData),
        withdrawals: rpcBlock.withdrawals,
        // uncleHeaders are not fetched and set here as provider methods for getting them are not supported
    };
}
exports.rpcToBlockData = rpcToBlockData;
//# sourceMappingURL=rpcToBlockData.js.map