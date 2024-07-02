var BlockchainDouble = require("../blockchain_double.js");
var Account = require("ethereumjs-account").default;
var Block = require("ethereumjs-block");
var Log = require("../utils/log.js");
var Receipt = require("../utils/receipt.js");
var utils = require("ethereumjs-util");
var ForkedStorageTrie = require("./forked_storage_trie.js");
var Web3 = require("web3");
var to = require("../utils/to.js");
var Transaction = require("../utils/transaction");
var async = require("async");
var LRUCache = require("lru-cache");
const Sublevel = require("level-sublevel");
const BN = utils.BN;

var inherits = require("util").inherits;

inherits(ForkedBlockchain, BlockchainDouble);

const httpReg = /^https?:/i;
const protocolReg = /^[A-Za-z][A-Za-z0-9+\-.]*:/;
const validProtocolReg = /^(?:http|ws)s?:/i;
const blockNumberReg = /@([0-9]+)$/;

function cloneWithoutId(obj) {
  return Object.assign({}, obj, { id: null });
}

function ForkedBlockchain(options) {
  this.options = options || {};

  if (options.fork == null || (typeof options.fork === "string" && options.fork.trim().length === 0)) {
    throw new Error("ForkedBlockchain must be passed a fork parameter.");
  }

  this.forkVersion = null;

  if (typeof options.fork === "string") {
    const blockNumber = blockNumberReg.exec(options.fork);

    if (blockNumber) {
      options.fork = options.fork.slice(0, blockNumber.index);
      options.fork_block_number = parseInt(blockNumber[1], 10);
    }

    let fork;
    if (!protocolReg.test(options.fork)) {
      // we don't have a protocol at all, assume ws
      options.fork = "ws://" + options.fork;
      fork = new Web3.providers.WebsocketProvider(options.fork);
    } else if (validProtocolReg.test(options.fork)) {
      if (httpReg.test(options.fork)) {
        fork = new Web3.providers.HttpProvider(options.fork);
      } else {
        fork = new Web3.providers.WebsocketProvider(options.fork);
      }
    } else {
      throw new Error(`Invalid scheme for fork url: ${options.fork}. Supported schemes are: http, https, ws, and wss.`);
    }

    this.fork = fork;
  } else {
    this.fork = options.fork;
  }

  this.forkBlockNumber = options.fork_block_number;
  this.forkCacheSize = parseInt(options.forkCacheSize);

  // if forkCacheSize is `0`, it means it is "off"
  if (!isNaN(this.forkCacheSize) && this.forkCacheSize !== 0) {
    const send = this.fork.send;
    const cache = new LRUCache({
      // `-1` means `Infinity`, which is represented by `0` in LRUCache's options
      max: this.forkCacheSize === -1 ? 0 : this.forkCacheSize,
      length: (bufValue, strKey) => {
        // compute the rough byte size of the stored key + value
        return Buffer.byteLength(bufValue) + Buffer.byteLength(strKey, "utf8");
      }
    });

    // Patch the `send` method of the underlying fork provider. We can
    // simply cache every non-error result because all requests to the
    // fork should be deterministic.
    const pendingRequests = new Map();
    this.fork.send = (payload, callback) => {
      let payloads;
      const sendArray = Array.isArray(payload);
      if (sendArray) {
        payloads = payload;
      } else {
        payloads = [payload];
      }
      Promise.all(
        payloads.map(async(payload) => {
          const key = JSON.stringify(cloneWithoutId(payload));
          let pendingRequest = pendingRequests.get(key);
          // if a request is in flight just wait for it instead of sending another
          // note: web3 actually polls on `.send`, resending the `payload`, so don't wait
          // if the new `payload` is the same as the `pendingRequest`.
          if (pendingRequest && pendingRequest.payload !== payload) {
            await pendingRequest.promise;
          }

          const cachedItem = cache.get(key);
          if (cachedItem) {
            const result = JSON.parse(cachedItem.toString());
            result.id = payload.id;
            return Promise.resolve({ error: null, result });
          } else {
            const promise = new Promise((resolve) => {
              send.call(this.fork, payload, (error, result) => {
                if (!error) {
                  cache.set(key, Buffer.from(JSON.stringify(cloneWithoutId(result))));
                }
                resolve({ error, result });
              });
            });
            pendingRequest = {
              payload,
              promise
            };

            pendingRequests.set(key, pendingRequest);
            // Node 8 doesn't have Promise.finally
            promise
              .catch(() => {})
              .then(() => {
                pendingRequests.delete(key);
              });
            return promise;
          }
        })
      ).then((errResults) => {
        if (!sendArray) {
          const errResult = errResults[0];
          callback(errResult.error, errResult.result);
        } else {
          let hasError = false;
          const errors = [];
          const results = [];
          errResults.forEach(({ error, result }) => {
            if (error) {
              hasError = true;
            }
            errors.push(error);
            results.push(result);
          });
          callback(hasError ? errors : null, results);
        }
      });
    };
  }

  this.time = options.time;
  this.storageTrieCache = {};

  BlockchainDouble.call(this, options);

  this.createVMFromStateTrie = function() {
    var vm = BlockchainDouble.prototype.createVMFromStateTrie.apply(this, arguments);
    this.patchVM(vm);
    return vm;
  };

  this.web3 = new Web3(this.fork);
  this._touchedKeys = [];
}

ForkedBlockchain.prototype.initialize = async function(accounts, callback) {
  try {
    const forkVersion = await new Promise((resolve, reject) => {
      this.web3.eth.net.getId((err, version) => {
        if (err) {
          if (this.options.network_id) {
            resolve(this.options.network_id);
          } else {
            Error.captureStackTrace(err);
            err.message = `The fork provider errored when checking net_version: ${err.message}`;
            reject(err);
          }
        } else {
          resolve(version);
        }
      });
    });

    this.forkVersion = forkVersion;

    const forkBlock = (this.forkBlock = await new Promise((resolve, reject) => {
      const queriedBlock = this.forkBlockNumber || "latest";
      this.web3.eth.getBlock(queriedBlock, (err, json) => {
        if (err) {
          Error.captureStackTrace(err);
          err.message =
            `The fork provider errored when checking for block '${
              queriedBlock
            }': ${err.message}`;
          reject(err);
        } else {
          resolve(json);
        }
      });
    }));

    // If no start time was passed, set the time to where we forked from.
    // We only want to do this if a block was explicitly passed. If a block
    // number wasn't passed, then we're using the last block and the current time.
    if (!this.time && this.forkBlockNumber) {
      this.time = this.options.time = new Date(to.number(forkBlock.timestamp) * 1000);
      this.setTime(this.time);
    }

    this.forkBlockNumber = this.options.fork_block_number = forkBlock.number;
    this.forkBlockHash = forkBlock.hash;

    // Fetch the nonce for all the accounts before we prime them in our state manager.
    // This is necessary to prevent conflicting contract deployments.
    const nonces = await Promise.all(
      accounts.map((account) => {
        return new Promise((resolve, reject) => {
          this.web3.eth.getTransactionCount(account.address, this.forkBlockNumber, (err, nonce) => {
            if (err) {
              Error.captureStackTrace(err);
              err.message =
                `The fork provider errored when checking the nonce for account ${
                  account.address
                }: ${err.message}`;
              reject(err);
            } else {
              resolve(nonce);
            }
          });
        });
      })
    );

    nonces.forEach((nonce, index) => {
      accounts[index].account.nonce = nonce;
    });

    BlockchainDouble.prototype.initialize.call(this, accounts, callback);
  } catch (err) {
    callback(err);
  }
};

ForkedBlockchain.prototype.patchVM = function(vm) {
  const trie = vm.stateManager._trie;
  const lookupAccount = this.getLookupAccount(trie);
  // Unfortunately forking requires a bit of monkey patching, but it gets the job done.
  vm.stateManager._cache._lookupAccount = lookupAccount;
  vm.stateManager._lookupStorageTrie = this.getLookupStorageTrie(trie, lookupAccount);
};

/**
 * @param db
 * @param root
 * @param options Allows overriding the options passed to the ForkedStorageTrie,
 * like `forkBlockNumber` (required for tracing transactions)
 */
ForkedBlockchain.prototype.createStateTrie = function(db, root, options) {
  options = Object.assign(
    {
      fork: this.fork,
      forkBlockNumber: this.forkBlockNumber,
      blockchain: this
    },
    options
  );
  // never allow the forkBlockNumber to go beyond our root forkBlockNumber
  if (options.forkBlockNumber > this.forkBlockNumber) {
    options.forkBlockNumber = this.forkBlockNumber;
  }
  return new ForkedStorageTrie(db, root, options);
};

ForkedBlockchain.prototype.createGenesisBlock = function(callback) {
  const forkBlock = this.forkBlock;

  this.createBlock(function(err, block) {
    if (err) {
      return callback(err);
    }

    block.header.number = forkBlock.number + 1;
    block.header.parentHash = forkBlock.hash;

    callback(null, block);
  });
};

ForkedBlockchain.prototype.getLookupStorageTrie = function(stateTrie, lookupAccount) {
  lookupAccount = lookupAccount || this.getLookupAccount(stateTrie);
  return (address, callback) => {
    const storageTrie = stateTrie.copy();
    storageTrie.address = address;
    lookupAccount(address, (err, account) => {
      if (err) {
        return callback(err);
      }

      storageTrie.root = account.stateRoot;
      callback(null, storageTrie);
    });
  };
};

ForkedBlockchain.prototype.isFallbackBlock = function(value, callback) {
  var self = this;

  self.getEffectiveBlockNumber(value, function(err, number) {
    if (err) {
      return callback(err);
    }

    callback(null, number <= to.number(self.forkBlockNumber));
  });
};

ForkedBlockchain.prototype.isBlockHash = function(value) {
  const isHash = typeof value === "string" && value.indexOf("0x") === 0 && value.length > 42;
  return isHash || (Buffer.isBuffer(value) && value.byteLength > 20);
};

ForkedBlockchain.prototype.isFallbackBlockHash = function(value, callback) {
  var self = this;

  if (!this.isBlockHash(value)) {
    return callback(null, false);
  }

  if (Buffer.isBuffer(value)) {
    value = to.hex(value);
  }

  self.data.blockHashes.get(value, function(err, blockIndex) {
    if (err) {
      if (err.notFound) {
        // If the block isn't found in our database, then it must be a fallback block.
        return callback(null, true);
      } else {
        return callback(err);
      }
    }
    callback(null, false);
  });
};

ForkedBlockchain.prototype.getFallbackBlock = function(numberOrHash, cb) {
  var self = this;

  if (Buffer.isBuffer(numberOrHash)) {
    // When tracing a transaction the VM sometimes ask for a block numbers as
    // buffers instead of numbers.
    numberOrHash = to.rpcDataHexString(numberOrHash);
  }
  if (typeof numberOrHash === "string" && numberOrHash.length !== 66) {
    numberOrHash = to.number(numberOrHash);
  }

  self.web3.eth.getBlock(numberOrHash, true, function(err, json) {
    if (err) {
      return cb(err);
    }

    if (json == null) {
      return cb(new Error("Block not found"));
    }

    var block = new Block();

    block.header.parentHash = utils.toBuffer(json.parentHash);
    block.header.uncleHash = utils.toBuffer(json.sha3Uncles);
    block.header.coinbase = utils.toBuffer(json.miner);
    block.header.stateRoot = utils.toBuffer(json.stateRoot); // Should we include the following three?
    block.header.transactionsTrie = utils.toBuffer(json.transactionsRoot);
    block.header.receiptTrie = utils.toBuffer(json.receiptsRoot);
    block.header.bloom = utils.toBuffer(json.logsBloom);
    block.header.difficulty = utils.toBuffer("0x" + json.totalDifficulty.toString(16)); // BigNumber
    block.header.number = utils.toBuffer(json.number);
    block.header.gasLimit = utils.toBuffer(json.gasLimit);
    block.header.gasUsed = utils.toBuffer(json.gasUsed);
    block.header.timestamp = utils.toBuffer(json.timestamp);
    block.header.extraData = utils.toBuffer(json.extraData);

    (json.transactions || []).forEach(function(txJson, index) {
      block.transactions.push(
        Transaction.fromJSON(txJson, Transaction.types.real, null, self.forkVersion, self.options.hardfork)
      );
    });

    // Fake block. Let's do the worst.
    // TODO: Attempt to fill out all block data so as to produce the same hash! (can we?)
    block.hash = function() {
      return utils.toBuffer(json.hash);
    };

    cb(null, block);
  });
};

ForkedBlockchain.prototype.getBlock = function(number, callback) {
  let checkFn;
  const isBlockHash = this.isBlockHash(number);
  if (isBlockHash) {
    checkFn = this.isFallbackBlockHash;
  } else {
    checkFn = this.isFallbackBlock;
  }
  checkFn.call(this, number, (err, isFallback) => {
    if (err) {
      return callback(err);
    }
    if (isFallback) {
      return this.getFallbackBlock(number, callback);
    }

    const getBlock = BlockchainDouble.prototype.getBlock.bind(this);
    if (isBlockHash) {
      getBlock(number, callback);
    } else {
      this.getRelativeBlockNumber(number, (err, number) => {
        if (err) {
          return callback(err);
        }
        getBlock(number, callback);
      });
    }
  });
};

ForkedBlockchain.prototype.getStorage = function(address, key, number, callback) {
  var self = this;

  this.getEffectiveBlockNumber(number, (err, blockNumber) => {
    if (err) {
      return callback(err);
    }

    if (blockNumber > self.forkBlockNumber) {
      // we should have this block

      self.getBlock(blockNumber, function(err, block) {
        if (err) {
          return callback(err);
        }

        const trie = self.stateTrie;

        // Manipulate the state root in place to maintain checkpoints
        const currentStateRoot = trie.root;
        self.stateTrie.root = block.header.stateRoot;

        self.getLookupStorageTrie(self.stateTrie)(address, (err, trie) => {
          if (err) {
            return callback(err);
          }

          trie.get(utils.setLengthLeft(utils.toBuffer(key), 32), function(err, value) {
            // Finally, put the stateRoot back for good
            trie.root = currentStateRoot;

            if (err != null) {
              return callback(err);
            }

            callback(null, value);
          });
        });
      });
    } else {
      // we're looking for something prior to forking, so let's
      // hit eth_getStorageAt
      self.web3.eth.getStorageAt(to.rpcDataHexString(address), to.rpcDataHexString(key), blockNumber, function(
        err,
        value
      ) {
        if (err) {
          return callback(err);
        }

        value = utils.rlp.encode(value);

        callback(null, value);
      });
    }
  });
};

ForkedBlockchain.prototype.getCode = function(address, number, callback) {
  var self = this;

  if (typeof number === "function") {
    callback = number;
    number = "latest";
  }

  if (!number) {
    number = "latest";
  }

  this.getEffectiveBlockNumber(number, function(err, effective) {
    if (err) {
      return callback(err);
    }
    number = effective;

    self.stateTrie.getTouchedAt(address, (err, touchedAt) => {
      if (err) {
        return callback(err);
      }

      if (typeof touchedAt !== "undefined" && touchedAt <= number) {
        BlockchainDouble.prototype.getCode.call(self, address, number, callback);
      } else {
        if (number > to.number(self.forkBlockNumber)) {
          number = "latest";
        }

        self.fetchCodeFromFallback(address, number, function(err, code) {
          if (code) {
            code = utils.toBuffer(code);
          }
          callback(err, code);
        });
      }
    });
  });
};

ForkedBlockchain.prototype.getLookupAccount = function(trie) {
  return (address, callback) => {
    // If the account doesn't exist in our state trie, get it off the wire.
    trie.keyExists(address, (err, exists) => {
      if (err) {
        return callback(err);
      }
      if (exists) {
        trie.get(address, (err, data) => {
          if (err) {
            return callback(err);
          }
          const account = new Account(data);
          callback(null, account);
        });
      } else {
        this.fetchAccountFromFallback(address, to.number(trie.forkBlockNumber), callback);
      }
    });
  };
};

ForkedBlockchain.prototype.getAccount = function(address, number, callback) {
  var self = this;

  if (typeof number === "function") {
    callback = number;
    number = "latest";
  }

  this.getEffectiveBlockNumber(number, function(err, effective) {
    if (err) {
      return callback(err);
    }
    number = effective;

    // If the account doesn't exist in our state trie, get it off the wire.
    self.stateTrie.keyExists(address, function(err, exists) {
      if (err) {
        return callback(err);
      }

      if (exists && number > to.number(self.forkBlockNumber)) {
        BlockchainDouble.prototype.getAccount.call(self, address, number, function(err, acc) {
          if (err) {
            return callback(err);
          }
          callback(null, acc);
        });
      } else {
        self.fetchAccountFromFallback(address, number, callback);
      }
    });
  });
};

ForkedBlockchain.prototype.getTransaction = function(hash, callback) {
  var self = this;
  BlockchainDouble.prototype.getTransaction.call(this, hash, function(err, tx) {
    if (err) {
      return callback(err);
    }
    if (tx != null) {
      return callback(null, tx);
    }

    self.web3.eth.getTransaction(hash, function(err, result) {
      if (err) {
        return callback(err);
      }

      if (result) {
        result = Transaction.fromJSON(result, Transaction.types.signed, null, self.forkVersion, self.options.hardfork);
      }

      callback(null, result);
    });
  });
};

ForkedBlockchain.prototype.getTransactionReceipt = function(hash, callback) {
  var self = this;
  BlockchainDouble.prototype.getTransactionReceipt.call(this, hash, function(err, receipt) {
    if (err) {
      return callback(err);
    }
    if (receipt) {
      return callback(null, receipt);
    }

    self.web3.eth.getTransactionReceipt(hash, function(err, receiptJson) {
      if (err) {
        return callback(err);
      }
      if (!receiptJson) {
        return callback();
      }

      async.parallel(
        {
          tx: self.getTransaction.bind(self, hash),
          block: self.getBlock.bind(self, receiptJson.blockNumber)
        },
        function(err, result) {
          if (err) {
            return callback(err);
          }

          var logs = receiptJson.logs.map(function(log) {
            log.block = result.block;
            return new Log(log);
          });

          var receipt = new Receipt(
            result.tx,
            result.block,
            logs,
            receiptJson.gasUsed,
            receiptJson.cumulativeGasUsed,
            receiptJson.contractAddress,
            receiptJson.status,
            to.hex(receiptJson.logsBloom)
          );

          callback(null, receipt);
        }
      );
    });
  });
};

ForkedBlockchain.prototype.fetchAccountFromFallback = function(address, blockNumber, callback) {
  var self = this;
  address = to.hex(address);

  async.parallel(
    {
      code: this.fetchCodeFromFallback.bind(this, address, blockNumber),
      balance: this.fetchBalanceFromFallback.bind(this, address, blockNumber),
      nonce: this.fetchNonceFromFallback.bind(this, address, blockNumber)
    },
    function(err, results) {
      if (err) {
        return callback(err);
      }

      var code = results.code;
      var balance = results.balance;
      var nonce = results.nonce;

      var account = new Account({
        nonce: nonce,
        balance: balance
      });

      // This puts the code on the trie, keyed by the hash of the code.
      // It does not actually link an account to code in the trie.
      account.setCode(self.stateTrie, utils.toBuffer(code), function(err) {
        if (err) {
          return callback(err);
        }
        callback(null, account);
      });
    }
  );
};

ForkedBlockchain.prototype.fetchCodeFromFallback = function(address, blockNumber, callback) {
  var self = this;
  address = to.hex(address);

  // Allow an optional blockNumber
  if (typeof blockNumber === "function") {
    callback = blockNumber;
    blockNumber = this.forkBlockNumber;
  }

  this.getSafeFallbackBlockNumber(blockNumber, function(err, safeBlockNumber) {
    if (err) {
      return callback(err);
    }

    self.web3.eth.getCode(address, safeBlockNumber, function(err, code) {
      if (err) {
        return callback(err);
      }

      code = "0x" + utils.toBuffer(code).toString("hex");
      callback(null, code);
    });
  });
};

ForkedBlockchain.prototype.fetchBalanceFromFallback = function(address, blockNumber, callback) {
  var self = this;
  address = to.hex(address);

  // Allow an optional blockNumber
  if (typeof blockNumber === "function") {
    callback = blockNumber;
    blockNumber = this.forkBlockNumber;
  }

  this.getSafeFallbackBlockNumber(blockNumber, function(err, safeBlockNumber) {
    if (err) {
      return callback(err);
    }

    self.web3.eth.getBalance(address, safeBlockNumber, function(err, balance) {
      if (err) {
        return callback(err);
      }

      balance = "0x" + new BN(balance).toString(16);
      callback(null, balance);
    });
  });
};

ForkedBlockchain.prototype.fetchNonceFromFallback = function(address, blockNumber, callback) {
  var self = this;
  address = to.hex(address);

  // Allow an optional blockNumber
  if (typeof blockNumber === "function") {
    callback = blockNumber;
    blockNumber = this.forkBlockNumber;
  }

  this.getSafeFallbackBlockNumber(blockNumber, function(err, safeBlockNumber) {
    if (err) {
      return callback(err);
    }

    self.web3.eth.getTransactionCount(address, safeBlockNumber, function(err, nonce) {
      if (err) {
        return callback(err);
      }

      nonce = "0x" + self.web3.utils.toBN(nonce).toString(16);
      callback(null, nonce);
    });
  });
};

ForkedBlockchain.prototype.getHeight = function(callback) {
  this.latestBlock(function(err, block) {
    if (err) {
      return callback(err);
    }
    callback(null, to.number(block.header.number));
  });
};

ForkedBlockchain.prototype.getRelativeBlockNumber = function(number, callback) {
  var self = this;
  this.getEffectiveBlockNumber(number, function(err, effective) {
    if (err) {
      return callback(err);
    }
    callback(null, effective - to.number(self.forkBlockNumber) - 1);
  });
};

ForkedBlockchain.prototype.getSafeFallbackBlockNumber = function(blockNumber, callback) {
  var forkBlockNumber = to.number(this.forkBlockNumber);

  if (blockNumber == null) {
    return callback(null, forkBlockNumber);
  }

  this.getEffectiveBlockNumber(blockNumber, function(err, effective) {
    if (err) {
      return callback(err);
    }
    if (effective > forkBlockNumber) {
      effective = forkBlockNumber;
    }

    callback(null, effective);
  });
};

ForkedBlockchain.prototype.getBlockLogs = function(number, callback) {
  var self = this;

  this.getEffectiveBlockNumber(number, function(err, effective) {
    if (err) {
      return callback(err);
    }

    self.getRelativeBlockNumber(effective, function(err, relative) {
      if (err) {
        return callback(err);
      }

      if (relative < 0) {
        self.getBlock(number, function(err, block) {
          if (err) {
            return callback(err);
          }

          self.web3.currentProvider.send(
            {
              jsonrpc: "2.0",
              method: "eth_getLogs",
              params: [
                {
                  fromBlock: to.hex(number),
                  toBlock: to.hex(number)
                }
              ],
              id: new Date().getTime()
            },
            function(err, res) {
              if (err) {
                return callback(err);
              }

              var logs = res.result.map(function(log) {
                // To make this result masquerade as the right information.
                log.block = block;
                return new Log(log);
              });

              callback(null, logs);
            }
          );
        });
      } else {
        BlockchainDouble.prototype.getBlockLogs.call(self, relative, callback);
      }
    });
  });
};

ForkedBlockchain.prototype.getQueuedNonce = function(address, callback) {
  var nonce = null;
  var addressBuffer = to.buffer(address);
  this.pending_transactions.forEach(function(tx) {
    if (!tx.from.equals(addressBuffer)) {
      return;
    }

    var pendingNonce = new BN(tx.nonce);
    // If this is the first queued nonce for this address we found,
    // or it's higher than the previous highest, note it.
    if (nonce === null || pendingNonce.gt(nonce)) {
      nonce = pendingNonce;
    }
  });

  // If we found a queued transaction nonce, return one higher
  // than the highest we found
  if (nonce != null) {
    return callback(null, nonce.iaddn(1).toArrayLike(Buffer));
  }
  this.getLookupAccount(this.stateTrie)(addressBuffer, function(err, account) {
    if (err) {
      return callback(err);
    }

    // nonces are initialized as an empty buffer, which isn't what we want.
    callback(null, account.nonce.length === 0 ? Buffer.from([0]) : account.nonce);
  });
};

ForkedBlockchain.prototype.processCall = function(tx, blockNumber, callback) {
  const self = this;

  this.getEffectiveBlockNumber(blockNumber, function(err, effectiveBlockNumber) {
    if (err) {
      return callback(err);
    }

    if (effectiveBlockNumber > self.forkBlockNumber) {
      BlockchainDouble.prototype.processCall.call(self, tx, blockNumber, callback);
    } else {
      self.web3.eth.call({
        from: to.rpcDataHexString(tx.from),
        to: to.nullableRpcDataHexString(tx.to),
        data: to.rpcDataHexString(tx.data)
      }, effectiveBlockNumber, function(err, result) {
        if (err) {
          return callback(err);
        }

        callback(null, {
          execResult: {
            returnValue: result
          }
        });
      });
    }
  });
};

ForkedBlockchain.prototype.processBlock = async function(vm, block, commit, callback) {
  const self = this;

  self._touchedKeys = [];
  BlockchainDouble.prototype.processBlock.call(self, vm, block, commit, callback);
};

ForkedBlockchain.prototype.putBlock = function(block, logs, receipts, callback) {
  const self = this;
  const touched = Sublevel(self.data.trie_db).sublevel("touched");
  const blockKey = `block-${to.number(block.header.number)}`;

  BlockchainDouble.prototype.putBlock.call(self, block, logs, receipts, function(err, result) {
    if (err) {
      return callback(err);
    }

    touched.put(blockKey, JSON.stringify(self._touchedKeys), (err) => {
      if (err) {
        return callback(err);
      }

      callback(null, result);
    });
  });
};

ForkedBlockchain.prototype.popBlock = function(callback) {
  const self = this;
  const touched = Sublevel(this.data.trie_db).sublevel("touched");

  this.data.blocks.last(function(err, block) {
    if (err) {
      return callback(err);
    }
    if (block == null) {
      return callback(null, null);
    }

    const blockKey = `block-${to.number(block.header.number)}`;
    touched.get(blockKey, function(err, value) {
      if (err) {
        return callback(err);
      }

      const touchedKeys = value ? JSON.parse(value) : [];
      async.eachSeries(
        touchedKeys,
        function(touchedKey, finished) {
          touched.del(touchedKey, finished);
        },
        function(err) {
          if (err) {
            return callback(err);
          }

          touched.del(blockKey, function(err) {
            if (err) {
              return callback(err);
            }

            BlockchainDouble.prototype.popBlock.call(self, callback);
          });
        }
      );
    });
  });
};

ForkedBlockchain.prototype.close = function(callback) {
  if (this.fork.disconnect) {
    this.fork.disconnect();
  }
  BlockchainDouble.prototype.close.call(this, callback);
};

module.exports = ForkedBlockchain;
