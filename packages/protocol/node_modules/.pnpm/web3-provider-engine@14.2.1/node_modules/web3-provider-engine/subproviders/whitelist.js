const inherits = require('util').inherits
const Subprovider = require('./subprovider.js')

module.exports = WhitelistProvider

inherits(WhitelistProvider, Subprovider)

function WhitelistProvider(methods){
  this.methods = methods;

  if (this.methods == null) {
    this.methods = [
      'eth_gasPrice',
      'eth_blockNumber',
      'eth_getBalance',
      'eth_getBlockByHash',
      'eth_getBlockByNumber',
      'eth_getBlockTransactionCountByHash',
      'eth_getBlockTransactionCountByNumber',
      'eth_getCode',
      'eth_getStorageAt',
      'eth_getTransactionByBlockHashAndIndex',
      'eth_getTransactionByBlockNumberAndIndex',
      'eth_getTransactionByHash',
      'eth_getTransactionCount',
      'eth_getTransactionReceipt',
      'eth_getUncleByBlockHashAndIndex',
      'eth_getUncleByBlockNumberAndIndex',
      'eth_getUncleCountByBlockHash',
      'eth_getUncleCountByBlockNumber',
      'eth_sendRawTransaction',
      'eth_getLogs'
    ];
  }
}

WhitelistProvider.prototype.handleRequest = function(payload, next, end){
  if (this.methods.indexOf(payload.method) >= 0) {
    next();
  } else {
    end(new Error("Method '" + payload.method + "' not allowed in whitelist."));
  }
}
