var to = require("./to.js");

// Expects:
//
// logIndex: ...
// transactionIndex: ...
// transactionHash: ...
// block: ...
// address: ...
// data: ...
// topics: ...
// type: ...

function Log(data) {
  var self = this;
  Object.keys(data).forEach(function(key) {
    self[key] = data[key];
  });
}

Log.prototype.toJSON = function() {
  // RPC quantity values like this.transactionIndex can be set to "0x00",
  // use the explicit rpcQuantityHexString to properly format the JSON, removing leading zeroes.
  // See RPC log format spec: https://github.com/ethereum/wiki/wiki/JSON-RPC
  return {
    logIndex: to.rpcQuantityHexString(this.logIndex),
    transactionIndex: to.rpcQuantityHexString(this.transactionIndex),
    transactionHash: to.rpcDataHexString(this.transactionHash),
    blockHash: to.rpcDataHexString(this.block.hash()),
    blockNumber: to.rpcQuantityHexString(this.block.header.number),
    address: to.rpcDataHexString(this.address),
    data: to.rpcDataHexString(this.data),
    topics: this.topics,
    type: "mined",
    removed: (this.removed || false)
  };
};

module.exports = Log;
