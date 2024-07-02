var utils = require("ethereumjs-util");
var inherits = require("util").inherits;
var StateManager = require("../statemanager.js");
var to = require("../utils/to");
var TXRejectedError = require("../utils/txrejectederror");

var blockHelper = require("../utils/block_helper");
var pkg = require("../../package.json");
const { BlockOutOfRangeError } = require("../utils/errorhelper");

var Subprovider = require("web3-provider-engine/subproviders/subprovider.js");

const maxSafeInt = "0x" + Number.MAX_SAFE_INTEGER.toString(16);
const vrs = ["v", "r", "s"];

inherits(GethApiDouble, Subprovider);

function GethApiDouble(options, provider) {
  var self = this;

  this.state = options.state || new StateManager(options, provider);
  this.options = options;
  this.initialized = false;

  this.initialization_error = null;
  this.post_initialization_callbacks = [];

  this.state.initialize(function(err) {
    if (err) {
      self.initialization_error = err;
    }
    self.initialized = true;

    var callbacks = self.post_initialization_callbacks;
    self.post_initialization_callbacks = [];

    callbacks.forEach(function(callback) {
      setImmediate(function() {
        callback(self.initialization_error, self.state);
      });
    });
  });
}

GethApiDouble.prototype.waitForInitialization = function(callback) {
  var self = this;
  if (self.initialized === false) {
    self.post_initialization_callbacks.push(callback);
  } else {
    callback(self.initialization_error, self.state);
  }
};

// Function to not pass methods through until initialization is finished
GethApiDouble.prototype.handleRequest = function(payload, next, end) {
  var self = this;

  if (self.initialization_error != null) {
    return end(self.initialization_error);
  }

  if (self.initialized === false) {
    self.waitForInitialization(self.getDelayedHandler(payload, next, end));
    return;
  }

  var method = self[payload.method];

  if (method == null) {
    return end(new Error("Method " + payload.method + " not supported."));
  }

  var params = payload.params || [];
  var args = [].concat(params);

  var addedBlockParam = false;

  if (self.requiresDefaultBlockParameter(payload.method) && args.length < method.length - 1) {
    args.push("latest");
    addedBlockParam = true;
  }

  args.push(end);

  // avoid crash by checking to make sure that we haven't specified too many arguments
  if (
    args.length > method.length ||
    (method.minLength !== undefined && args.length < method.minLength) ||
    (method.minLength === undefined && args.length < method.length)
  ) {
    var errorMessage = `Incorrect number of arguments. Method '${payload.method}' requires `;
    if (method.minLength) {
      errorMessage += `between ${method.minLength - 1} and ${method.length - 1} arguments. `;
    } else {
      errorMessage += `exactly ${method.length - 1} arguments. `;
    }

    if (addedBlockParam) {
      errorMessage += "Including the implicit block argument, r";
    } else {
      // new sentence, capitalize it.
      errorMessage += "R";
    }
    errorMessage += `equest specified ${args.length - 1} arguments: ${JSON.stringify(args)}.`;

    return end(new Error(errorMessage));
  }

  method.apply(self, args);
};

GethApiDouble.prototype.getDelayedHandler = function(payload, next, end) {
  var self = this;
  return function(err, state) {
    if (err) {
      end(err);
    }
    self.handleRequest(payload, next, end);
  };
};

GethApiDouble.prototype.requiresDefaultBlockParameter = function(method) {
  // object for O(1) lookup.
  var methods = {
    eth_getBalance: true,
    eth_getCode: true,
    eth_getTransactionCount: true,
    eth_getStorageAt: true,
    eth_call: true,
    eth_estimateGas: true
  };

  return methods[method] === true;
};

// Handle individual requests.

GethApiDouble.prototype.eth_accounts = function(callback) {
  callback(null, Object.keys(this.state.accounts));
};

GethApiDouble.prototype.eth_blockNumber = function(callback) {
  this.state.blockNumber(function(err, result) {
    if (err) {
      return callback(err);
    }
    callback(null, to.hex(result));
  });
};

GethApiDouble.prototype.eth_chainId = function(callback) {
  callback(null, to.hex(this.options._chainIdRpc));
};

GethApiDouble.prototype.eth_coinbase = function(callback) {
  callback(null, this.state.coinbase);
};

GethApiDouble.prototype.eth_mining = function(callback) {
  callback(null, this.state.is_mining);
};

GethApiDouble.prototype.eth_hashrate = function(callback) {
  callback(null, "0x0");
};

GethApiDouble.prototype.eth_gasPrice = function(callback) {
  callback(null, utils.addHexPrefix(this.state.gasPrice()));
};

GethApiDouble.prototype.eth_getBalance = function(address, blockNumber, callback) {
  this.state.getBalance(address, blockNumber, callback);
};

GethApiDouble.prototype.eth_getCode = function(address, blockNumber, callback) {
  this.state.getCode(address, blockNumber, callback);
};

GethApiDouble.prototype.eth_getBlockByNumber = function(blockNumber, includeFullTransactions, callback) {
  this.state.blockchain.getBlock(blockNumber, function(err, block) {
    if (err) {
      if (err instanceof BlockOutOfRangeError) {
        return callback(null, null);
      } else {
        return callback(err);
      }
    }

    callback(null, blockHelper.toJSON(block, includeFullTransactions));
  });
};

GethApiDouble.prototype.eth_getBlockByHash = function(txHash, includeFullTransactions, callback) {
  this.eth_getBlockByNumber.apply(this, arguments);
};

GethApiDouble.prototype.eth_getBlockTransactionCountByNumber = function(blockNumber, callback) {
  this.state.blockchain.getBlock(blockNumber, function(err, block) {
    if (err) {
      if (err instanceof BlockOutOfRangeError) {
        // block doesn't exist
        return callback(null, null);
      }
      return callback(err);
    }
    callback(null, to.rpcQuantityHexString(block.transactions.length));
  });
};

GethApiDouble.prototype.eth_getBlockTransactionCountByHash = function(blockHash, callback) {
  this.eth_getBlockTransactionCountByNumber.apply(this, arguments);
};

GethApiDouble.prototype.eth_getTransactionReceipt = function(hash, callback) {
  this.state.getTransactionReceipt(hash, function(err, receipt) {
    if (err) {
      return callback(err);
    }

    var result = null;

    if (receipt && receipt.block) {
      result = receipt.toJSON();
      vrs.forEach((element) => {
        delete result[element];
      });
    }
    callback(null, result);
  });
};

GethApiDouble.prototype.eth_getTransactionByHash = function(hash, callback) {
  this.state.getTransactionReceipt(hash, function(err, receipt) {
    if (err) {
      return callback(err);
    }

    var result = null;

    if (receipt) {
      // if there is no block then the transaction is still pending
      if (!receipt.block) {
        // assemble the block object with null values for pending transactions
        receipt.block = {
          transactions: [],
          hash: () => {
            return null;
          },
          header: { number: null }
        };

        result = receipt.tx.toJsonRpc(receipt.block);
      } else {
        result = receipt.tx.toJsonRpc(receipt.block);
      }
    }
    callback(null, result);
  });
};

GethApiDouble.prototype.eth_getTransactionByBlockHashAndIndex = function(hashOrNumber, index, callback) {
  index = to.number(index);

  this.state.getBlock(hashOrNumber, function(err, block) {
    if (err) {
      // block doesn't exist by that hash
      if (err.notFound) {
        return callback(null, null);
      } else {
        return callback(err);
      }
    }

    if (index >= block.transactions.length) {
      return callback(new Error("Transaction at index " + to.hex(index) + " does not exist in block."));
    }

    var tx = block.transactions[index];
    var result = tx.toJsonRpc(block);

    callback(null, result);
  });
};

GethApiDouble.prototype.eth_getTransactionByBlockNumberAndIndex = function(hashOrNumber, index, callback) {
  this.eth_getTransactionByBlockHashAndIndex(hashOrNumber, index, callback);
};

GethApiDouble.prototype.eth_getTransactionCount = function(address, blockNumber, callback) {
  this.state.getTransactionCount(address, blockNumber, (err, count) => {
    if (err instanceof BlockOutOfRangeError) {
      return callback(null, null);
    }
    return callback(err, count);
  });
};

GethApiDouble.prototype.eth_sign = function(address, dataToSign, callback) {
  var result;
  var error;

  try {
    result = this.state.sign(address, dataToSign);
  } catch (e) {
    error = e;
  }

  callback(error, result);
};

GethApiDouble.prototype.eth_signTypedData = function(address, typedDataToSign, callback) {
  var result;
  var error;

  try {
    result = this.state.signTypedData(address, typedDataToSign);
  } catch (e) {
    error = e;
  }

  callback(error, result);
};

GethApiDouble.prototype.eth_sendTransaction = function(txData, callback) {
  this.state.queueTransaction("eth_sendTransaction", txData, null, callback);
};

GethApiDouble.prototype.eth_sendRawTransaction = function(rawTx, callback) {
  let data;
  if (rawTx) {
    data = to.buffer(rawTx);
  }

  if (data === undefined) {
    throw new TXRejectedError("rawTx must be a string, JSON-encoded Buffer, or Buffer.");
  }

  this.state.queueRawTransaction(data, callback);
};

GethApiDouble.prototype._setCallGasLimit = function(txData) {
  // if the caller didn't specify a gas limit make sure we set one
  if (!txData.gas && !txData.gasLimit) {
    const globalCallGasLimit = this.options.callGasLimit;
    if (globalCallGasLimit != null) {
      // if the user configured a global `callGasLimit` use it
      txData.gas = globalCallGasLimit;
    } else {
      // otherwise, set a very high gas limit. We'd use Infinity, or some VM flag to ignore gasLimit checks like
      // geth does, but the VM doesn't currently support that for `runTx`.
      // https://github.com/ethereumjs/ethereumjs-vm/blob/4bbb6e394a344717890d618a6be1cf67b8e5b74d/lib/runTx.ts#L71
      txData.gas = maxSafeInt;
    }
  }
};
GethApiDouble.prototype.eth_call = function(txData, blockNumber, callback) {
  this._setCallGasLimit(txData);
  this.state.queueTransaction("eth_call", txData, blockNumber, callback);
};

GethApiDouble.prototype.eth_estimateGas = function(txData, blockNumber, callback) {
  this._setCallGasLimit(txData);
  this.state.queueTransaction("eth_estimateGas", txData, blockNumber, callback);
};

GethApiDouble.prototype.eth_getStorageAt = function(address, position, blockNumber, callback) {
  this.state.queueStorage(address, position, blockNumber, callback);
};

GethApiDouble.prototype.eth_newBlockFilter = function(callback) {
  var filterId = utils.addHexPrefix(utils.intToHex(this.state.latestFilterId));
  this.state.latestFilterId += 1;
  callback(null, filterId);
};

GethApiDouble.prototype.eth_getFilterChanges = function(filterId, callback) {
  var blockHash = this.state
    .latestBlock()
    .hash()
    .toString("hex");
  // Mine a block after each request to getFilterChanges so block filters work.
  this.state.mine();
  callback(null, [blockHash]);
};

GethApiDouble.prototype.eth_getLogs = function(filter, callback) {
  this.state.getLogs(filter, callback);
};

GethApiDouble.prototype.eth_uninstallFilter = function(filterId, callback) {
  callback(null, true);
};

GethApiDouble.prototype.eth_protocolVersion = function(callback) {
  callback(null, "63");
};

GethApiDouble.prototype.bzz_hive = function(callback) {
  callback(null, []);
};

GethApiDouble.prototype.bzz_info = function(callback) {
  callback(null, []);
};

GethApiDouble.prototype.shh_version = function(callback) {
  callback(null, "2");
};

GethApiDouble.prototype.eth_getCompilers = function(callback) {
  callback(null, []);
};

GethApiDouble.prototype.eth_syncing = function(callback) {
  callback(null, false);
};

GethApiDouble.prototype.net_listening = function(callback) {
  callback(null, true);
};

GethApiDouble.prototype.net_peerCount = function(callback) {
  callback(null, 0);
};

GethApiDouble.prototype.web3_clientVersion = function(callback) {
  callback(null, "EthereumJS TestRPC/v" + pkg.version + "/ethereum-js");
};

GethApiDouble.prototype.web3_sha3 = function(string, callback) {
  callback(null, to.hex(utils.keccak256(string)));
};

GethApiDouble.prototype.net_version = function(callback) {
  // net_version returns a string containing a base 10 integer.
  callback(null, this.state.net_version + "");
};

GethApiDouble.prototype.miner_start = function(threads, callback) {
  if (!callback && typeof threads === "function") {
    callback = threads;
    threads = null;
  }

  this.state.startMining(function(err) {
    callback(err, true);
  });
};

// indicate that `miner_start` only requires one argument (the callback)
GethApiDouble.prototype.miner_start.minLength = 1;

GethApiDouble.prototype.miner_stop = function(callback) {
  this.state.stopMining(function(err) {
    callback(err, true);
  });
};

GethApiDouble.prototype.rpc_modules = function(callback) {
  // returns the availible api modules and versions
  callback(null, { eth: "1.0", net: "1.0", rpc: "1.0", web3: "1.0", evm: "1.0", personal: "1.0" });
};

GethApiDouble.prototype.personal_listAccounts = function(callback) {
  callback(null, Object.keys(this.state.personal_accounts));
};

GethApiDouble.prototype.personal_newAccount = function(password, callback) {
  var account = this.state.createAccount({ generate: true });
  this.state.accounts[account.address.toLowerCase()] = account;
  this.state.personal_accounts[account.address.toLowerCase()] = true;
  this.state.account_passwords[account.address.toLowerCase()] = password;
  callback(null, account.address);
};

GethApiDouble.prototype.personal_importRawKey = function(rawKey, password, callback) {
  var account = this.state.createAccount({ secretKey: rawKey });
  this.state.accounts[account.address.toLowerCase()] = account;
  this.state.personal_accounts[account.address.toLowerCase()] = true;
  this.state.account_passwords[account.address.toLowerCase()] = password;
  callback(null, account.address);
};

GethApiDouble.prototype.personal_lockAccount = function(address, callback) {
  var account = this.state.personal_accounts[address.toLowerCase()];
  if (account !== true) {
    var error = "Account not found";
    return callback(error);
  }
  delete this.state.unlocked_accounts[address.toLowerCase()];
  callback(null, true);
};

GethApiDouble.prototype.personal_unlockAccount = function(address, password, duration, callback) {
  // FIXME handle duration
  var account = this.state.personal_accounts[address.toLowerCase()];
  if (account !== true) {
    var accountError = "Account not found";
    return callback(accountError);
  }

  var storedPassword = this.state.account_passwords[address.toLowerCase()];
  if (storedPassword !== undefined && storedPassword !== password) {
    var passwordError = "Invalid password";
    return callback(passwordError);
  }

  this.state.unlocked_accounts[address.toLowerCase()] = true;
  callback(null, true);
};

GethApiDouble.prototype.personal_sendTransaction = function(txData, password, callback) {
  if (txData.from == null) {
    var error = "Sender not found";
    callback(error);
    return;
  }

  var from = utils.addHexPrefix(txData.from).toLowerCase();

  var self = this;
  self.personal_unlockAccount(from, password, null, function(err) {
    if (err) {
      return callback(err);
    }

    self.state.queueTransaction("eth_sendTransaction", txData, null, function(err, ret) {
      self.state.unlocked_accounts[from.toLowerCase()] = false;
      callback(err, ret);
    });
  });
};

/* Functions for testing purposes only. */

GethApiDouble.prototype.evm_snapshot = function(callback) {
  this.state.snapshot(callback);
};

GethApiDouble.prototype.evm_revert = function(snapshotId, callback) {
  this.state.revert(snapshotId, callback);
};

GethApiDouble.prototype.evm_increaseTime = function(seconds, callback) {
  callback(null, this.state.blockchain.increaseTime(seconds));
};

GethApiDouble.prototype.evm_setTime = function(date, callback) {
  callback(null, this.state.blockchain.setTime(new Date(date)));
};

GethApiDouble.prototype.evm_mine = function(timestamp, callback) {
  if (typeof timestamp === "function") {
    callback = timestamp;
    timestamp = undefined;
  }
  this.state.processBlock(timestamp, function(err) {
    if (err) {
      return callback(err);
    }
    callback(err, "0x0");
  });
};

// indicate that `evm_mine` only requires one argument (the callback)
GethApiDouble.prototype.evm_mine.minLength = 1;

/**
 * Unlocks any unknown account.
 *
 * Note: accounts known to the `personal` namespace and accounts returned by
 * `eth_accounts` cannot be unlocked using this method.
 *
 * @param {*} address the address of the account to unlock
 * @param {*} callback
 * @returns `true` if the account was unlocked successfully, `false` if the
 * account was already unlocked. Throws an error if the account could not be
 * unlocked.
 */
GethApiDouble.prototype.evm_unlockUnknownAccount = function(address, callback) {
  // check if given address is already unlocked
  address = address.toLowerCase();
  if (this.state.unlocked_accounts[address] !== true) {
    if (this.state.personal_accounts[address] !== true) {
      // unlock the account
      this.state.unlocked_accounts[address] = true;
      return callback(null, true);
    } else {
      // if the account is known to the `personal` namespace we may not unlock
      // it
      return callback(new Error("cannot unlock known/personal account"));
    }
  }
  // account was already unlocked, return `false`
  callback(null, false);
};

/**
 * Locks any unknown account.
 *
 * Note: accounts known to the `personal` namespace and accounts returned by
 * `eth_accounts` cannot be locked using this method.
 *
 * @param {*} address the address of the account to lock
 * @param {*} callback
 * @returns `true` if the account was locked successfully, `false` if the
 * account was already locked. Throws an error if the account could not be
 * locked.
 */
GethApiDouble.prototype.evm_lockUnknownAccount = function(address, callback) {
  // Checks if given address is already unlocked
  address = address.toLowerCase();
  if (this.state.unlocked_accounts[address]) {
    if (this.state.personal_accounts[address]) {
      // if the account is known to the `personal` namespace we may not lock it
      return callback(new Error("cannot lock known/personal account"));
    } else {
      // unlock the account
      delete this.state.unlocked_accounts[address];
      return callback(null, true);
    }
  }
  // account wasn't locked to begin with, return `false`
  callback(null, false);
};

GethApiDouble.prototype.debug_traceTransaction = function(txHash, params, callback) {
  if (typeof params === "function") {
    callback = params;
    params = [];
  }

  this.state.queueTransactionTrace(txHash, params, callback);
};

/*
  RPC AUDIT:
  TODO ETH: eth_getUncleCountByBlockHash, eth_getUncleCountByBlockNumber, eth_getUncleByBlockHashAndIndex,
        eth_getUncleByBlockNumberAndIndex, eth_getWork, eth_submitWork, eth_submitHashrate

  TODO DB: db_putString, db_getString, db_putHex, db_getHex

  TODO WHISPER: shh_post, shh_newIdentity, shh_hasIdentity, shh_newGroup, shh_addToGroup,
        shh_newFilter, shh_uninstallFilter, shh_getFilterChanges, shh_getMessages
*/

/**
 * Returns the number of uncles in a block from a block matching the given block hash.
 *
 * @param {DATA, 32 Bytes} hash - hash of a block.
 * @callback callback
 * @param {error} err - Error Object
 * @param {QUANTITY} result - integer of the number of uncles in this block.
 */
GethApiDouble.prototype.eth_getUncleCountByBlockHash = function(hash, callback) {
  callback(null, "0x0");
};

/**
 * Returns the number of uncles in a block from a block matching the given block number.
 *
 * @param {QUANTITY} blockNumber -
 *  ^integer of a block number, or the string "latest", "earliest" or "pending". Ex: '0xe8', // 232
 * @callback callback
 * @param {error} err - Error Object
 * @param {QUANTITY} result - integer of the number of uncles in this block.
 */
GethApiDouble.prototype.eth_getUncleCountByBlockNumber = function(blockNumber, callback) {
  callback(null, "0x0");
};

/**
 * Returns information about a uncle of a block by hash and uncle index position.
 *
 * @param {DATA, 32 Bytes} hash - hash of a block
 * @param {QUANTITY} index - the uncle's index position.
 * @callback callback
 * @param {error} err - Error Object
 * @param {Object} result - A block object,
 */
GethApiDouble.prototype.eth_getUncleByBlockHashAndIndex = function(hash, index, callback) {
  callback(null, {});
};

/**
 * Returns information about a uncle of a block by number and uncle index position.
 *
 * @param {QUANTITY} blockNumber -
 * ^a block number, or the string "earliest", "latest" or "pending", as in the default block parameter.
 * @param {QUANTITY} uncleIndex - the uncle's index position.
 * @callback callback
 * @param {error} err - Error object
 * @param {Object} resutl - A block object,
 */
GethApiDouble.prototype.eth_getUncleByBlockNumberAndIndex = function(blockNumber, uncleIndex, callback) {
  callback(null, {});
};

/**
 * Returns: An Array with the following elements
 * 1: DATA, 32 Bytes - current block header pow-hash
 * 2: DATA, 32 Bytes - the seed hash used for the DAG.
 * 3: DATA, 32 Bytes - the boundary condition ("target"), 2^256 / difficulty.
 *
 * @param {QUANTITY} filterId - A filter id
 * @callback callback
 * @param {error} err - Error object
 * @param {Array} result - the hash of the current block, the seedHash, and the boundary condition to be met ("target").
 */
GethApiDouble.prototype.eth_getWork = function(filterId, callback) {
  callback(null, []);
};

/**
 * Used for submitting a proof-of-work solution
 *
 * @param {DATA, 8 Bytes} nonce - The nonce found (64 bits)
 * @param {DATA, 32 Bytes} powHash - The header's pow-hash (256 bits)
 * @param {DATA, 32 Bytes} digest - The mix digest (256 bits)
 * @callback callback
 * @param {error} err - Error object
 * @param {Boolean} result - returns true if the provided solution is valid, otherwise false.
 */
GethApiDouble.prototype.eth_submitWork = function(nonce, powHash, digest, callback) {
  callback(null, false);
};

/**
 * Used for submitting mining hashrate.
 *
 * @param {String} hashRate - a hexadecimal string representation (32 bytes) of the hash rate
 * @param {String} clientID - A random hexadecimal(32 bytes) ID identifying the client
 * @callback callback
 * @param {error} err - Error object
 * @param {Boolean} result - returns true if submitting went through succesfully and false otherwise.
 */
GethApiDouble.prototype.eth_submitHashrate = function(hashRate, clientID, callback) {
  callback(null, false);
};

/**
 * Stores a string in the local database.
 *
 * @param {String} dbName - Database name.
 * @param {String} key - Key name.
 * @param {String} value - String to store.
 * @callback callback
 * @param {error} err - Error object
 * @param {Boolean} result - returns true if the value was stored, otherwise false.
 */
GethApiDouble.prototype.db_putString = function(dbName, key, value, callback) {
  callback(null, false);
};

/**
 * Returns string from the local database
 *
 * @param {String} dbName - Database name.
 * @param {String} key - Key name.
 * @callback callback
 * @param {error} - Error Object
 * @param {String} result - The previously stored string.
 */
GethApiDouble.prototype.db_getString = function(dbName, key, callback) {
  callback(null, "");
};

/**
 * Stores binary data in the local database.
 *
 * @param {String} dbName - Database name.
 * @param {String} key - Key name.
 * @param {DATA} data - Data to store.
 * @callback callback
 * @param {error} err - Error Object
 * @param {Boolean} result - returns true if the value was stored, otherwise false.
 */
GethApiDouble.prototype.db_putHex = function(dbName, key, data, callback) {
  callback(null, false);
};

/**
 * Returns binary data from the local database
 *
 * @param {String} dbName - Database name.
 * @param {String} key - Key name.
 * @callback callback
 * @param {error} err - Error Object
 * @param {DATA} result - The previously stored data.
 */
GethApiDouble.prototype.db_getHex = function(dbName, key, callback) {
  callback(null, "0x00");
};

/**
 * Sends a whisper message.
 *
 * @param {DATA, 60 Bytes} from - (optional) The identity of the sender.
 * @param {DATA, 60 Bytes} to -
 *  ^(optional) The identity of the receiver. When present whisper will encrypt the message so that
 *  only the receiver can decrypt it.
 * @param {Array of DATA} topics - Array of DATA topics, for the receiver to identify messages.
 * @param {DATA} payload - The payload of the message.
 * @param {QUANTITY} priority - The integer of the priority in a range from ... (?).
 * @param {QUANTITY} ttl - integer of the time to live in seconds.
 * @callback callback
 * @param {error} err - Error Object
 * @param {Boolean} result - returns true if the message was sent, otherwise false.
 */
GethApiDouble.prototype.shh_post = function(from, to, topics, payload, priority, ttl, callback) {
  callback(null, false);
};

/**
 * Creates new whisper identity in the client.
 *
 * @callback callback
 * @param {error} err - Error Object
 * @param {DATA, 60 Bytes} result - the address of the new identiy.
 */
GethApiDouble.prototype.shh_newIdentity = function(callback) {
  callback(null, "0x00");
};

/**
 * Checks if the client hold the private keys for a given identity.
 *
 * @param {DATA, 60 Bytes} address - The identity address to check.
 * @callback callback
 * @param {error} err - Error Object
 * @param {Boolean} result - returns true if the client holds the privatekey for that identity, otherwise false.
 */
GethApiDouble.prototype.shh_hasIdentity = function(address, callback) {
  callback(null, false);
};

/**
 * Creates a new group.
 *
 * @callback callback
 * @param {error} err - Error Object
 * @param {DATA, 60 Bytes} result - the address of the new group.
 */
GethApiDouble.prototype.shh_newGroup = function(callback) {
  callback(null, "0x00");
};

/**
 * Adds a whisper identity to the group
 *
 * @param {DATA, 60 Bytes} - The identity address to add to a group.
 * @callback callback
 * @param {error} err - Error Object
 * @param {Boolean} result - returns true if the identity was successfully added to the group, otherwise false.
 */
GethApiDouble.prototype.shh_addToGroup = function(address, callback) {
  callback(null, false);
};

/**
 * Creates filter to notify, when client receives whisper message matching the filter options.
 *
 * @param {DATA, 60 Bytes} to -
 * ^(optional) Identity of the receiver. When present it will try to decrypt any incoming message
 *  if the client holds the private key to this identity.
 * @param {Array of DATA} topics - Array of DATA topics which the incoming message's topics should match.
 * @callback callback
 * @param {error} err - Error Object
 * @param {Boolean} result - returns true if the identity was successfully added to the group, otherwise false.
 */
GethApiDouble.prototype.shh_newFilter = function(to, topics, callback) {
  callback(null, false);
};

/**
 * Uninstalls a filter with given id. Should always be called when watch is no longer needed.
 * Additonally Filters timeout when they aren't requested with shh_getFilterChanges for a period of time.
 *
 * @param {QUANTITY} id - The filter id. Ex: "0x7"
 * @callback callback
 * @param {error} err - Error Object
 * @param {Boolean} result - true if the filter was successfully uninstalled, otherwise false.
 */
GethApiDouble.prototype.shh_uninstallFilter = function(id, callback) {
  callback(null, false);
};

/**
 * Polling method for whisper filters. Returns new messages since the last call of this method.
 *
 * @param {QUANTITY} id - The filter id. Ex: "0x7"
 * @callback callback
 * @param {error} err - Error Object
 * @param {Array} result - More Info: https://github.com/ethereum/wiki/wiki/JSON-RPC#shh_getfilterchanges
 */
GethApiDouble.prototype.shh_getFilterChanges = function(id, callback) {
  callback(null, []);
};

/**
 * Get all messages matching a filter. Unlike shh_getFilterChanges this returns all messages.
 *
 * @param {QUANTITY} id - The filter id. Ex: "0x7"
 * @callback callback
 * @param {error} err - Error Object
 * @param {Array} result - See: shh_getFilterChanges
 */
GethApiDouble.prototype.shh_getMessages = function(id, callback) {
  callback(null, false);
};

module.exports = GethApiDouble;
