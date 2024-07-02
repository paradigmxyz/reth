var to = require("./to");

module.exports = {
  toJSON: function(block, includeFullTransactions) {
    return {
      number: to.rpcQuantityHexString(block.header.number),
      hash: to.hex(block.hash()),
      parentHash: to.hex(block.header.parentHash), // common.hash
      mixHash: to.hex(block.header.mixHash),
      nonce: to.rpcDataHexString(to.hex(block.header.nonce), 16),
      sha3Uncles: to.hex(block.header.uncleHash),
      logsBloom: to.hex(block.header.bloom),
      transactionsRoot: to.hex(block.header.transactionsTrie),
      stateRoot: to.hex(block.header.stateRoot),
      receiptsRoot: to.hex(block.header.receiptTrie),
      miner: to.hex(block.header.coinbase),
      difficulty: to.rpcQuantityHexString(block.header.difficulty),
      totalDifficulty: to.rpcQuantityHexString(block.header.difficulty), // TODO: Figure out what to do here.
      extraData: to.rpcDataHexString(block.header.extraData),
      size: to.hex(1000), // TODO: Do something better here
      gasLimit: to.rpcQuantityHexString(block.header.gasLimit),
      gasUsed: to.rpcQuantityHexString(block.header.gasUsed),
      timestamp: to.rpcQuantityHexString(block.header.timestamp),
      transactions: block.transactions.map(function(tx) {
        if (includeFullTransactions) {
          return tx.toJsonRpc(block);
        } else {
          return to.hex(tx.hash());
        }
      }),
      uncles: [] // block.uncleHeaders.map(function(uncleHash) {return to.hex(uncleHash)})
    };
  }
};
