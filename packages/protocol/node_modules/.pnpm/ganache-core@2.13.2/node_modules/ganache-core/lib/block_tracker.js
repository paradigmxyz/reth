// this replaces `eth-block-tracker` in the provider-engine, as that block tracker is meant to work with
// an external provider instance

const EventEmitter = require("events");
var blockHelper = require("./utils/block_helper");

function GanacheBlockTracker(opts) {
  opts = opts || {};
  EventEmitter.apply(this);
  if (!opts.blockchain) {
    throw new Error("RpcBlockTracker - no blockchain specified.");
  }
  if (!opts.blockchain.on) {
    throw new Error("RpcBlockTracker - blockchain is not an EventEmitter.");
  }
  this._blockchain = opts.blockchain;
  this.start = this.start.bind(this);
  this.stop = this.stop.bind(this);
  this.getTrackingBlock = this.getTrackingBlock.bind(this);
  this.awaitCurrentBlock = this.awaitCurrentBlock.bind(this);
  this._setCurrentBlock = this._setCurrentBlock.bind(this);
}

GanacheBlockTracker.prototype = Object.create(EventEmitter.prototype);
GanacheBlockTracker.prototype.constructor = GanacheBlockTracker;

GanacheBlockTracker.prototype.getTrackingBlock = function() {
  return this._currentBlock;
};

GanacheBlockTracker.prototype.getCurrentBlock = function() {
  return this._currentBlock;
};

GanacheBlockTracker.prototype.awaitCurrentBlock = function() {
  const self = this;
  // return if available
  if (this._currentBlock) {
    return this._currentBlock;
  }
  // wait for "sync" event
  return new Promise((resolve) => this.once("block", resolve)).then(() => self._currentBlock);
};

GanacheBlockTracker.prototype.start = function(opts = {}) {
  this._blockchain.on("block", this._setCurrentBlock);
  return Promise.resolve();
};

GanacheBlockTracker.prototype.stop = function() {
  this._isRunning = false;
  this._blockchain.removeListener("block", this._setCurrentBlock);
};

//
// private
//

GanacheBlockTracker.prototype._setCurrentBlock = function(newBlock) {
  const block = blockHelper.toJSON(newBlock, true);
  if (this._currentBlock && this._currentBlock.hash === block.hash) {
    return;
  }
  const oldBlock = this._currentBlock;
  this._currentBlock = block;
  this.emit("latest", block);
  this.emit("sync", { block, oldBlock });
  this.emit("block", block);
};

module.exports = GanacheBlockTracker;
