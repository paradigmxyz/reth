var to = require("./to");

function Receipt(tx, block, logs, gasUsed, cumulativeGasUsed, contractAddress, status, logsBloom) {
  this.tx = tx;
  this.block = block;
  this.logs = logs;
  this.gasUsed = gasUsed;
  this.cumulativeGasUsed = cumulativeGasUsed;
  this.contractAddress = contractAddress;
  this.status = status;
  this.logsBloom = logsBloom;

  this.transactionIndex = 0;

  this.txHash = tx.hash();

  for (var i = 0; i < block.transactions.length; i++) {
    var current = block.transactions[i];
    if (current.hash().equals(this.txHash)) {
      this.transactionIndex = i;
      break;
    }
  }
}

Receipt.prototype.toJSON = function() {
  // Enforce Hex formatting as defined in the RPC spec.
  return {
    transactionHash: to.rpcDataHexString(this.txHash),
    transactionIndex: to.rpcQuantityHexString(this.transactionIndex),
    blockHash: to.rpcDataHexString(this.block.hash()),
    blockNumber: to.rpcQuantityHexString(this.block.header.number),
    from: to.rpcDataHexString(this.tx.from),
    to: to.nullableRpcDataHexString(this.tx.to),
    gasUsed: to.rpcQuantityHexString(this.gasUsed),
    cumulativeGasUsed: to.rpcQuantityHexString(this.cumulativeGasUsed),
    contractAddress: this.contractAddress != null ? to.rpcDataHexString(this.contractAddress) : null,
    logs: this.logs.map(function(log) {
      return log.toJSON();
    }),
    status: to.rpcQuantityHexString(this.status),
    logsBloom: to.rpcDataHexString(this.logsBloom),
    v: to.rpcQuantityHexString(this.tx.v),
    r: to.rpcQuantityHexString(this.tx.r),
    s: to.rpcQuantityHexString(this.tx.s)
  };
};

module.exports = Receipt;
