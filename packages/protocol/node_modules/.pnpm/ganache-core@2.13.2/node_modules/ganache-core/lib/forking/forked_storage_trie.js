const Sublevel = require("level-sublevel");
const MerklePatriciaTree = require("merkle-patricia-tree");
const BaseTrie = require("merkle-patricia-tree/baseTrie");
const checkpointInterface = require("merkle-patricia-tree/checkpoint-interface");
var utils = require("ethereumjs-util");
var inherits = require("util").inherits;
var Web3 = require("web3");
var to = require("../utils/to.js");

inherits(ForkedStorageBaseTrie, BaseTrie);

function ForkedStorageBaseTrie(db, root, options) {
  BaseTrie.call(this, db, root);
  this._touched = Sublevel(this.db).sublevel("touched");

  this.options = options;
  this.address = options.address;
  this.forkBlockNumber = options.forkBlockNumber;
  this.blockchain = options.blockchain;
  this.fork = options.fork;
  this.web3 = new Web3(this.fork);
  this.persist = typeof options.persist === "undefined" ? true : options.persist;
}

// Note: This overrides a standard method whereas the other methods do not.
ForkedStorageBaseTrie.prototype.get = function(key, callback) {
  var self = this;

  key = utils.toBuffer(key);

  self.keyExists(key, function(err, keyExists) {
    if (err) {
      return callback(err);
    }

    self.getTouchedAt(key, function(err, touchedAt) {
      if (err) {
        return callback(err);
      }

      if (keyExists && typeof touchedAt !== "undefined") {
        MerklePatriciaTree.prototype.get.call(self, key, function(err, r) {
          callback(err, r);
        });
      } else {
        // If this is the main trie, get the whole account.
        if (self.address == null) {
          self.blockchain.fetchAccountFromFallback(key, self.forkBlockNumber, function(err, account) {
            if (err) {
              return callback(err);
            }

            callback(null, account.serialize());
          });
        } else {
          self.web3.eth.getStorageAt(
            to.rpcDataHexString(self.address),
            to.rpcDataHexString(key),
            self.forkBlockNumber,
            function(err, value) {
              if (err) {
                return callback(err);
              }

              value = to.rpcQuantityBuffer(value);

              callback(null, value);
            }
          );
        }
      }
    });
  });
};

ForkedStorageBaseTrie.prototype.keyExists = function(key, callback) {
  key = utils.toBuffer(key);
  this.findPath(key, (err, node, remainder, stack) => {
    const exists = node && remainder.length === 0;
    callback(err, exists);
  });
};

ForkedStorageBaseTrie.prototype.touch = function(key, callback) {
  if (!this.persist) {
    return callback();
  }

  const self = this;
  let rpcKey = to.rpcDataHexString(key);
  if (this.address) {
    rpcKey = `${to.rpcDataHexString(this.address)};${rpcKey}`;
  }
  rpcKey = rpcKey.toLowerCase();

  this._touched.get(rpcKey, (err, result) => {
    if (err && err.type !== "NotFoundError") {
      return callback(err);
    }

    if (typeof result === "undefined") {
      // key doesn't exist
      this.blockchain.data.blocks.last((err, lastBlock) => {
        if (err) {
          return callback(err);
        }

        const number = lastBlock === null ? self.forkBlockNumber : to.number(lastBlock.header.number);
        this._touched.put(rpcKey, number + 1);
        this.blockchain._touchedKeys.push(rpcKey);
        callback();
      });
    } else {
      callback();
    }
  });
};

const originalPut = ForkedStorageBaseTrie.prototype.put;
ForkedStorageBaseTrie.prototype.put = function(key, value, callback) {
  const self = this;
  this.touch(key, function(err) {
    if (err) {
      return callback(err);
    }

    originalPut.call(self, key, value, callback);
  });
};

ForkedStorageBaseTrie.prototype.getTouchedAt = function(key, callback) {
  let rpcKey = to.rpcDataHexString(key);
  if (this.address) {
    rpcKey = `${to.rpcDataHexString(this.address)};${rpcKey}`;
  }
  rpcKey = rpcKey.toLowerCase();

  this._touched.get(rpcKey, function(err, result) {
    if (err && err.type !== "NotFoundError") {
      return callback(err);
    }

    callback(null, result);
  });
};

ForkedStorageBaseTrie.prototype.del = function(key, callback) {
  this.put(key, 0, callback);
};

ForkedStorageBaseTrie.prototype.copy = function() {
  return new ForkedStorageBaseTrie(this.db, this.root, this.options);
};

inherits(ForkedStorageTrie, ForkedStorageBaseTrie);

function ForkedStorageTrie(db, root, options) {
  ForkedStorageBaseTrie.call(this, db, root, options);
  checkpointInterface.call(this, this);
}

ForkedStorageTrie.prove = MerklePatriciaTree.prove;
ForkedStorageTrie.verifyProof = MerklePatriciaTree.verifyProof;

module.exports = ForkedStorageTrie;
