var Account = require("ethereumjs-account").default;
var RuntimeError = require("./utils/runtimeerror");
var Transaction = require("./utils/transaction");
var utils = require("ethereumjs-util");
var seedrandom = require("seedrandom");
var bip39 = require("bip39");
var wallet = require("ethereumjs-wallet");
var hdkey = require("ethereumjs-wallet/hdkey");
var async = require("async");
var BlockchainDouble = require("./blockchain_double.js");
var ForkedBlockchain = require("./forking/forked_blockchain.js");
var Web3 = require("web3");
var fs = require("fs");
var sigUtil = require("eth-sig-util");
var _ = require("lodash");
const { BlockOutOfRangeError } = require("./utils/errorhelper");
const BN = utils.BN;
const rlp = require("rlp");
const Common = require("ethereumjs-common").default;

const ZERO_BUFFER = Buffer.from([0]);

var to = require("./utils/to");
var random = require("./utils/random");
var TXRejectedError = require("./utils/txrejectederror");

function StateManager(options, provider) {
  this.options = options = this._applyDefaultOptions(options || {});

  if (options.fork) {
    this.blockchain = new ForkedBlockchain(options);
  } else {
    this.blockchain = new BlockchainDouble(options);
  }

  this.vm = this.blockchain.vm;
  this.stateTrie = this.blockchain.stateTrie;

  this.accounts = {};
  this.secure = !!options.secure;
  this.account_passwords = {};
  this.personal_accounts = {};
  this.total_accounts = options.total_accounts;
  this.coinbase = null;

  this.latest_filter_id = 1;

  // This queue manages actions that shouldn't be run in parallel.
  // The action_processing flag ensures new actions are queued instead of
  // run immediately.
  this.action_queue = [];
  this.action_processing = false;

  this.snapshots = [];
  this.logger = options.logger;
  this.net_version = options.network_id;
  this.mnemonic = options.mnemonic;
  this.wallet = hdkey.fromMasterSeed(bip39.mnemonicToSeed(this.mnemonic));
  this.wallet_hdpath = options.hd_path || options.hdPath;

  this.gasPriceVal = to.rpcQuantityHexString(options.gasPrice);

  this.is_mining = true;
  this.blockTime = options.blockTime;
  this.is_mining_on_interval = !!options.blockTime;
  this.mining_interval_timeout = null;

  this._provider = provider;
}

const defaultOptions = {
  forkCacheSize: 1024 * 1024 * 1024,
  total_accounts: 10,
  gasPrice: "0x77359400", // 2 gwei
  default_balance_ether: 100,
  unlocked_accounts: [],
  hdPath: "m/44'/60'/0'/0/"
};

StateManager.prototype._applyDefaultOptions = function(options) {
  // do this so that we can use the same seed on our next run and get the same
  // results without explicitly setting a seed up front
  if (!options.seed) {
    options.seed = random.randomAlphaNumericString(10, seedrandom());
  }

  // generate a randomized default mnemonic
  if (!options.mnemonic) {
    const randomBytes = random.randomBytes(16, seedrandom(options.seed));
    options.mnemonic = bip39.entropyToMnemonic(randomBytes.toString("hex"));
  }

  if (!options.fork && !options.network_id) {
    options.network_id = new Date().getTime();
  }

  // We want this function to mutate the options object so that we can report
  // our settings back to our consumer application (e.g., ganache)
  return _.merge(options, defaultOptions, Object.assign({}, options));
};

StateManager.prototype.initialize = function(callback) {
  var self = this;

  var accounts = [];

  const defaultBalanceWei = to.hex(Web3.utils.toWei(self.options.default_balance_ether.toString(), "ether"));

  if (self.options.accounts) {
    accounts = self.options.accounts.map(self.createAccount.bind(self));
  } else {
    if (!self.total_accounts) {
      return callback(
        new Error("Cannot initialize chain: either options.accounts or options.total_accounts must be specified")
      );
    }

    for (var i = 0; i < self.total_accounts; i++) {
      accounts.push(
        self.createAccount({
          index: i,
          balance: defaultBalanceWei
        })
      );
    }
  }

  self.coinbase = to.hex(accounts[0].address);
  self.accounts = {};

  accounts.forEach(function(data) {
    self.accounts[data.address] = data;
    self.personal_accounts[data.address.toLowerCase()] = true;
  });

  // Turn array into object, mostly for speed purposes.
  // No need for caller to specify private keys.
  self.unlocked_accounts = self.options.unlocked_accounts.reduce(function(obj, address) {
    // If it doesn't have a hex prefix, must be a number (either a string or number type).
    if ((address + "").indexOf("0x") !== 0) {
      const idx = parseInt(address);
      const account = accounts[idx];
      if (!account) {
        throw new Error(`Account at index ${idx} not found. Max index available is ${accounts.length - 1}.`);
      }
      address = account.address.toLowerCase();
    }

    obj[address.toLowerCase()] = true; // can be any value
    return obj;
  }, {});

  if (!self.secure) {
    accounts.forEach(function(data) {
      self.unlocked_accounts[data.address.toLowerCase()] = data;
    });
  }

  if (self.options.account_keys_path) {
    const fileData = {
      addresses: {},
      private_keys: {}
    };
    accounts.forEach(function(account) {
      fileData.private_keys[account.address] = account.secretKey.toString("hex");
      fileData.addresses[account.address] = account;
    });
    const json = JSON.stringify(fileData);
    fs.writeFileSync(self.options.account_keys_path, json, "utf8");
  }

  self.blockchain.initialize(accounts, function(err) {
    if (err) {
      return callback(err);
    }

    // If the user didn't pass a specific version id in, then use the
    // forked blockchain's version (if it exists) or create our own.
    if (!self.net_version) {
      self.net_version = self.blockchain.forkVersion;
    }

    if (self.is_mining_on_interval) {
      self.mineOnInterval();
    }
    callback();
  });
};

StateManager.prototype._minerCancellationToken = null;
StateManager.prototype.mineOnInterval = function() {
  // cancel the a previous miner's timeout
  clearTimeout(this.mining_interval_timeout);

  // make sure a pending eth_mine doesn't come back and execute mineOnInterval
  // again...
  if (this._minerCancellationToken !== null) {
    this._minerCancellationToken.cancelled = true;
  }

  // if mining was stopped `mineOnInterval` shouldn't start mining again
  if (!this.is_mining) {
    this.logger.log("Warning: mineOnInterval called when miner was stopped");
    return;
  }

  const cancellationToken = { cancelled: false };
  this._minerCancellationToken = cancellationToken;

  const timeout = (this.mining_interval_timeout = setTimeout(
    this._provider.send.bind(this._provider),
    this.blockTime * 1000,
    { method: "evm_mine" },
    () => {
      if (!cancellationToken.cancelled) {
        this.mineOnInterval.bind(this)();
      }
    }
  ));

  // Ensure this won't keep a node process open.
  if (typeof timeout.unref === "function") {
    timeout.unref();
  }
};

StateManager.prototype.createAccount = function(opts, i) {
  var secretKey;

  if (opts.generate) {
    secretKey = wallet.generate().getPrivateKey();
  } else if (opts.secretKey) {
    secretKey = utils.toBuffer(to.hex(opts.secretKey));
  } else {
    var index = typeof opts.index === "undefined" ? i : opts.index;
    var acct = this.wallet.derivePath(this.wallet_hdpath + index); // index is a number
    secretKey = acct.getWallet().getPrivateKey(); // Buffer
  }

  var publicKey = utils.privateToPublic(secretKey);
  var address = utils.publicToAddress(publicKey);

  var account = new Account();

  account.balance = to.hex(opts.balance);

  var data = {
    secretKey: secretKey,
    publicKey: publicKey,
    address: to.hex(address).toLowerCase(),
    account: account
  };

  return data;
};

StateManager.prototype.blockNumber = function(callback) {
  return this.blockchain.getHeight(callback);
};

StateManager.prototype.gasPrice = function() {
  return this.gasPriceVal;
};

StateManager.prototype.getBalance = function(address, number, callback) {
  this.blockchain.getBalance(address, number, function(err, balance) {
    if (balance) {
      balance = to.rpcQuantityHexString(balance);
    }
    callback(err, balance);
  });
};

StateManager.prototype.getTransactionCount = function(address, number, callback) {
  this.blockchain.getNonce(address, number, function(err, nonce) {
    if (nonce) {
      nonce = to.rpcQuantityHexString(nonce);
    }
    callback(err, nonce);
  });
};

StateManager.prototype.getCode = function(address, number, callback) {
  this.blockchain.getCode(address, number, function(err, code) {
    if (code) {
      code = to.hex(code);
    }
    callback(err, code);
  });
};

StateManager.prototype.queueRawTransaction = function(data, callback) {
  // a hack until we get chainid and networkId alignment
  // in the next major (breaking_change) version:
  // Reason: historically we didn't validate chain ids.
  let chainId;
  if (Buffer.isBuffer(data)) {
    const decodedData = rlp.decode(data);
    let v = decodedData[6];
    if (v !== undefined) {
      v = utils.bufferToInt(v);
      chainId = Math.floor((v - 35) / 2);
      if (chainId < 0) {
        chainId = 0;
      }
    }
  }
  const common = !chainId
    ? this.blockchain.vm.opts.common
    : Common.forCustomChain(
      "mainnet", // TODO needs to match chain id
      {
        name: "ganache",
        networkId: this.options.network_id || this.blockchain.forkVersion,
        chainId,
        comment: "Local test network",
        bootstrapNodes: []
      },
      this.options.hardfork
    );
  const tx = new Transaction(data, Transaction.types.signed, common);
  // use toLowerCase() to properly handle from addresses meant to be validated.
  const from = to.hex(tx.from).toLowerCase();
  this._queueTransaction("eth_sendRawTransaction", tx, from, null, callback);
};

StateManager.prototype.queueStorage = function(address, position, block, callback) {
  this.action_queue.push({
    method: "eth_getStorageAt",
    address: utils.addHexPrefix(address),
    position: utils.addHexPrefix(position),
    block: block,
    callback: callback
  });

  // We know there's work, so get started.
  this.processNextAction();
};

StateManager.prototype.queueTransaction = function(method, txJsonRpc, blockNumber, callback) {
  // use toLowerCase() to properly handle from addresses meant to be validated.
  const from = txJsonRpc.from ? to.hex(txJsonRpc.from).toLowerCase() : null;

  if (from == null) {
    callback(new TXRejectedError("from not found; is required"));
    return;
  }

  // Error checks. It's possible to JSON.stringify a Buffer to JSON.
  // we actually now handle this "properly" (not sure about spec), but for
  // legacy reasons we don't allow it.
  if (txJsonRpc.to && typeof txJsonRpc.to !== "string") {
    return callback(new TXRejectedError("Invalid to address"));
  }

  const hasOwnProperty = Object.prototype.hasOwnProperty;
  const isKnownAccount = hasOwnProperty.call(this.accounts, from);

  if (method === "eth_sendTransaction" && !hasOwnProperty.call(this.unlocked_accounts, from)) {
    const msg = isKnownAccount ? "signer account is locked" : "sender account not recognized";
    return callback(new TXRejectedError(msg));
  }

  let type = Transaction.types.none;
  if (!isKnownAccount || method === "eth_call" || method === "eth_estimateGas") {
    type |= Transaction.types.fake;
  }

  let tx;
  try {
    tx = Transaction.fromJSON(txJsonRpc, type, this.blockchain.vm.opts.common);
    this._setTransactionDefaults(tx, method === "eth_sendTransaction");
  } catch (e) {
    callback(e);
    return;
  }
  this._queueTransaction(method, tx, from, blockNumber, callback);
};

StateManager.prototype._setTransactionDefaults = function(tx, isTransaction) {
  if (isTransaction && tx.gasLimit.length === 0) {
    tx.gasLimit = utils.toBuffer(this.blockchain.defaultTransactionGasLimit);
  }

  if (tx.gasPrice.length === 0) {
    tx.gasPrice = utils.toBuffer(this.gasPriceVal);
  }

  if (tx.value.length === 0) {
    tx.value = Buffer.from([0]);
  }

  if (tx.to.length === 0 || tx.to.equals(ZERO_BUFFER)) {
    tx.to = Buffer.allocUnsafe(0);
  }
};

StateManager.prototype._queueTransaction = function(method, tx, from, blockNumber, callback) {
  if (!(tx instanceof Transaction)) {
    throw new TXRejectedError("tx must be of type Transaction");
  }

  // If the transaction has a higher gas limit than the block gas limit, error.
  if (
    (method === "eth_sendRawTransaction" || method === "eth_sendTransaction") &&
    to.number(tx.gasLimit) > to.number(this.blockchain.blockGasLimit)
  ) {
    return callback(new TXRejectedError("Exceeds block gas limit"));
  }

  this.action_queue.push({
    method,
    from,
    tx,
    callback,
    blockNumber
  });

  // We know there's work, so get started.
  this.processNextAction();
};

StateManager.prototype.queueTransactionTrace = function(txHash, params, callback) {
  this.action_queue.push({
    method: "debug_traceTransaction",
    hash: to.hex(txHash),
    params: params,
    callback: callback
  });

  // We know there's work, so get started.
  this.processNextAction();
};

StateManager.prototype.processNextAction = function(override) {
  var self = this;

  if (override !== true) {
    if (this.action_processing === true || this.action_queue.length === 0) {
      return;
    }
  }

  var queued = this.action_queue.shift();

  // Set the flag that we're currently processing something.
  this.action_processing = true;

  var intermediary = function(err, result) {
    queued.callback(err, result);

    if (self.action_queue.length > 0) {
      self.processNextAction(true);
    } else {
      self.action_processing = false;
    }
  };

  if (typeof queued.method === "function") {
    var result = queued.method();
    return intermediary(null, result);
  } else if (queued.method === "eth_getStorageAt") {
    this.blockchain.getStorage(queued.address, queued.position, queued.block, function(err, result) {
      if (err) {
        return intermediary(err);
      }

      if (result) {
        result = utils.rlp.decode(result);
      }

      result = to.hex(result || 0);
      intermediary(null, result);
    });
  } else if (queued.method === "debug_traceTransaction") {
    this.blockchain.processTransactionTrace(queued.hash, queued.params, intermediary);
  } else if (queued.method === "eth_sendTransaction" || queued.method === "eth_sendRawTransaction") {
    this.processTransaction(queued.from, queued.tx, intermediary);
  } else if (queued.method === "eth_call") {
    this.processCall(queued.from, queued.tx, queued.blockNumber, intermediary);
  } else if (queued.method === "eth_estimateGas") {
    this.processGasEstimate(queued.from, queued.tx, queued.blockNumber, intermediary);
  }
};

StateManager.prototype.sign = function(address, dataToSign) {
  var account = this.accounts[to.hex(address).toLowerCase()];

  if (!account) {
    throw new Error("cannot sign data; no private key");
  }

  var secretKey = account.secretKey;
  var msg = to.buffer(dataToSign, "hex");
  var msgHash = utils.hashPersonalMessage(msg);
  var sgn = utils.ecsign(msgHash, Buffer.from(secretKey));
  // ethereumjs-utils changed the behavior of toRpcSig so that the `v` value
  // output by `toRpcSig` is always `sgn.v - 27` (basically `0` or `1`). In
  // order to avoid a breaking change in Ganache at this time we are calculating
  // a chain ID that will always a) validate the v, and b) generate an output v
  // of `0` or `1`, like it used to.
  const v = sgn.v - 27;
  const chainId = (v - 35) / 2;
  return utils.toRpcSig(v, sgn.r, sgn.s, chainId);
};

StateManager.prototype.signTypedData = function(address, typedDataToSign) {
  var account = this.accounts[to.hex(address).toLowerCase()];
  if (!account) {
    throw new Error("cannot sign data; no private key");
  }

  if (!typedDataToSign.types) {
    throw new Error("cannot sign data; types missing");
  }

  if (!typedDataToSign.types.EIP712Domain) {
    throw new Error("cannot sign data; EIP712Domain definition missing");
  }

  if (!typedDataToSign.domain) {
    throw new Error("cannot sign data; domain missing");
  }

  if (!typedDataToSign.primaryType) {
    throw new Error("cannot sign data; primaryType missing");
  }

  if (!typedDataToSign.message) {
    throw new Error("cannot sign data; message missing");
  }

  return sigUtil.signTypedData_v4(account.secretKey, { data: typedDataToSign });
};

StateManager.prototype.printTransactionReceipt = function(txHash, error, callback) {
  var self = this;

  self.blockchain.getTransactionReceipt(txHash, function(err, receipt) {
    if (err) {
      return callback(err);
    }

    self.blockchain.latestBlock(function(err, block) {
      if (err) {
        return callback(err);
      }

      receipt = receipt.toJSON();

      self.logger.log("");
      self.logger.log("  Transaction: " + txHash);

      if (receipt.contractAddress != null) {
        self.logger.log("  Contract created: " + receipt.contractAddress);
      }

      self.logger.log("  Gas usage: " + parseInt(receipt.gasUsed, 16));
      self.logger.log("  Block Number: " + parseInt(receipt.blockNumber, 16));
      self.logger.log("  Block Time: " + new Date(to.number(block.header.timestamp) * 1000).toString());

      if (error) {
        self.logger.log("  Runtime Error: " + error.error);
        if (error.reason) {
          self.logger.log("  Revert reason: " + error.reason);
        }
      }

      self.logger.log("");

      callback(null, txHash);
    });
  });
};

StateManager.prototype.processBlock = function(timestamp, callback) {
  var self = this;

  if (typeof timestamp === "function") {
    callback = timestamp;
    timestamp = null;
  }

  self.blockchain.processNextBlock(timestamp, function(runtimeError, transactions, vmOutput) {
    if (runtimeError && runtimeError instanceof RuntimeError === false) {
      // This is bad. Get out.
      return callback(runtimeError, transactions, vmOutput);
    }

    // TODO: Can we refactor printTransactionReceipt so it's synchronous?
    // We technically have the raw vm receipts (though they're not full receipts here...).
    async.eachSeries(
      transactions,
      function(tx, finishedPrinting) {
        var hash = to.hex(tx.hash());
        var error = runtimeError == null ? { results: {} } : runtimeError;
        self.printTransactionReceipt(hash, error.results[hash], finishedPrinting);
      },
      callback(runtimeError, transactions, vmOutput)
    );
  });
};

StateManager.prototype.processBlocks = function(totalBlocks, callback) {
  var self = this;

  if (typeof totalBlocks === "function") {
    callback = totalBlocks;
    totalBlocks = null;
  }

  // Note: VM errors (errors that the VM directly returns) trump all runtime errors.
  var runtimeError = null;
  var amountProcessed = 0;

  async.whilst(
    function() {
      var shouldContinue;

      if (totalBlocks == null) {
        shouldContinue = self.blockchain.pending_transactions.length > 0;
      } else {
        shouldContinue = amountProcessed < totalBlocks;
      }

      return shouldContinue;
    },
    function(done) {
      self.processBlock(function(err, transactions, vmOutput) {
        amountProcessed += 1;

        if (err) {
          if (err instanceof RuntimeError === false) {
            // This is bad. Get out.
            return done(err);
          }

          // We must have a RuntimeError. Merge results if we've found
          // other runtime errors during this execution.
          if (runtimeError == null) {
            runtimeError = err;
          } else {
            runtimeError.combine(err);
          }
        }

        // Note we don't quit on runtime errors. We keep processing transactions.
        done();
      });
    },
    function(err) {
      // Remember: vm errors trump runtime errors
      callback(err || runtimeError);
    }
  );
};

StateManager.prototype.processCall = function(from, tx, blockNumber, callback) {
  var self = this;

  function next(err, tx) {
    if (err) {
      return callback(err);
    }

    self.blockchain.processCall(tx, blockNumber, function(err, results) {
      if (err) {
        if (err instanceof BlockOutOfRangeError) {
          // block doesn't exist
          return callback(null, null);
        }
        return callback(err);
      }

      var result = "0x";
      if (!results.error && results.execResult.returnValue) {
        result = to.hex(results.execResult.returnValue);
      } else if (results.error) {
        self.logger.log(`Error processing call: ${results.error}`);
      }

      return callback(null, result);
    });
  }

  // `eth_call` should never need to validate an existing nonce
  if (tx.nonce.length === 0) {
    self.createTransactionWithCorrectNonce(tx, from, next);
  } else {
    next(null, tx);
  }
};

StateManager.prototype.processGasEstimate = function(from, tx, blockNumber, callback) {
  var self = this;

  function next(err, tx) {
    if (err) {
      return callback(err);
    }

    self.blockchain.estimateGas(tx, blockNumber, function(err, results) {
      if (err) {
        return callback(err);
      }
      var result = "0x";
      if (!results.error) {
        result = to.hex(results.gasEstimate);
      } else {
        self.logger.log(`Error calculating gas estimate: ${results.error}`);
      }
      return callback(null, result);
    });
  }

  // `eth_estimateGas` should never need to validate an existing nonce
  if (tx.nonce.length === 0) {
    self.createTransactionWithCorrectNonce(tx, from, next);
  } else {
    next(null, tx);
  }
};

StateManager.prototype.processTransaction = function(from, tx, callback) {
  var self = this;

  self.createTransactionWithCorrectNonce(tx, from, function(err, tx) {
    if (err) {
      return callback(err);
    }

    self.blockchain.queueTransaction(tx);

    var txHash = to.hex(tx.hash());

    // If we're not currently mining or we're mining on an interval,
    // only queue the transaction, don't process it.
    if (self.is_mining === false || self.is_mining_on_interval) {
      return callback(null, txHash);
    }

    self.processBlocks(function(err) {
      if (err) {
        return callback(err);
      }
      callback(null, txHash);
    });
  });
};

StateManager.prototype.getTransactionReceipt = function(hash, callback) {
  this.blockchain.getTransactionReceipt(hash, function(err, receipt) {
    if (err && err.notFound) {
      // Return null if the receipt's not found.
      return callback(null, null);
    }
    callback(err, receipt);
  });
};

StateManager.prototype.getBlock = function(hashOrNumber, callback) {
  this.blockchain.getBlock(hashOrNumber, callback);
};

StateManager.prototype.getLogs = function(filter, callback) {
  var self = this;

  // filter.address may be a single address or an array
  // https://github.com/ethereum/wiki/wiki/JSON-RPC#eth_getlogs
  var expectedAddress = filter.address && (Array.isArray(filter.address) ? filter.address : [filter.address]);
  expectedAddress =
    expectedAddress &&
    expectedAddress.map(function(a) {
      return a.toLowerCase();
    });
  var expectedTopics = filter.topics || [];

  async.parallel(
    {
      fromBlock: this.blockchain.getEffectiveBlockNumber.bind(this.blockchain, filter.fromBlock || "latest"),
      toBlock: this.blockchain.getEffectiveBlockNumber.bind(this.blockchain, filter.toBlock || "latest"),
      latestBlock: this.blockchain.getEffectiveBlockNumber.bind(this.blockchain, "latest"),
      block: this.blockchain.getBlock.bind(this.blockchain, filter.blockHash || 0)
    },
    function(err, results) {
      if (err) {
        return callback(err);
      }
      var fromBlock = results.fromBlock;
      var toBlock = results.toBlock;
      var latestBlock = results.latestBlock;
      var block = results.block;

      if (toBlock > latestBlock) {
        toBlock = latestBlock;
      }

      if (filter.blockHash) {
        fromBlock = to.number(block.header.number);
        toBlock = to.number(block.header.number);
      }

      var logs = [];
      var current = fromBlock;

      async.whilst(
        function() {
          return current <= toBlock;
        },
        function(finished) {
          self.blockchain.getBlockLogs(current, function(err, blockLogs) {
            if (err) {
              return finished(err);
            }

            // Filter logs that match the address
            var filtered = !expectedAddress
              ? blockLogs
              : blockLogs.filter(function(log) {
                return expectedAddress.indexOf(log.address.toLowerCase()) > -1;
              });

            // Now filter based on topics.
            filtered = filtered.filter(function(log) {
              var keep = true;
              for (var i = 0; i < expectedTopics.length; i++) {
                var expectedTopic = expectedTopics[i];
                var logTopic = log.topics[i];
                if (expectedTopic == null) {
                  continue;
                }
                var isMatch = Array.isArray(expectedTopic)
                  ? expectedTopic.includes(logTopic)
                  : expectedTopic === logTopic;
                if (i >= log.topics.length || !isMatch) {
                  keep = false;
                  break;
                }
              }
              return keep;
            });

            logs.push.apply(logs, filtered);

            current += 1;
            finished();
          });
        },
        function(err) {
          if (err) {
            return callback(err);
          }

          logs = logs.map(function(log) {
            return log.toJSON();
          });

          callback(err, logs);
        }
      );
    }
  );
};

// Note: Snapshots have 1-based ids.
StateManager.prototype.snapshot = function(callback) {
  var self = this;

  this.blockchain.getHeight(function(err, blockNumber) {
    if (err) {
      return callback(err);
    }

    self.snapshots.push({
      blockNumber: blockNumber,
      timeAdjustment: self.blockchain.timeAdjustment
    });

    self.logger.log("Saved snapshot #" + self.snapshots.length);

    callback(null, to.hex(self.snapshots.length));
  });
};

StateManager.prototype.revert = function(snapshotId, callback) {
  var self = this;

  if (snapshotId === null || snapshotId === undefined) {
    callback(new Error("invalid snapshotId"));
    return;
  }
  // Convert from hex.
  try {
    snapshotId = utils.bufferToInt(snapshotId);
  } catch (e) {
    callback(e);
    return;
  }

  this.logger.log("Reverting to snapshot #" + snapshotId);

  if (snapshotId > this.snapshots.length || snapshotId <= 0) {
    // the snapshot doesn't exist now, or it has already been reverted
    callback(null, false);
    return false;
  }

  // Convert to zero based.
  snapshotId = snapshotId - 1;
  var timeAdjustment = this.snapshots[snapshotId].timeAdjustment;

  // Loop through each snapshot with a higher id than the current one.
  async.whilst(
    function() {
      return self.snapshots.length > snapshotId;
    },
    function(nextSnapshot) {
      var snapshot = self.snapshots.pop();

      // For each snapshot, asynchronously pop off the blocks it represents.
      async.during(
        function(doneWithTest) {
          self.blockchain.getHeight(function(err, blockNumber) {
            if (err) {
              return doneWithTest(err);
            }

            doneWithTest(null, blockNumber > snapshot.blockNumber);
          });
        },
        function(nextBlock) {
          self.blockchain.popBlock(function(err) {
            if (err) {
              return nextBlock(err);
            }
            nextBlock();
          });
        },
        nextSnapshot
      );
    },
    function(err) {
      if (err) {
        return callback(err);
      }

      // Pending transactions are removed when you revert.
      self.blockchain.clearPendingTransactions();
      // The time adjustment is restored to its prior state
      self.blockchain.timeAdjustment = timeAdjustment;

      callback(null, true);
    }
  );
};

StateManager.prototype.hasContractCode = function(address, callback) {
  this.vm.stateManager.getContractCode(address, function(err, result) {
    if (err != null) {
      callback(err, false);
    } else {
      callback(null, true);
    }
  });
};

StateManager.prototype.startMining = function(callback) {
  if (this.is_mining) {
    process.nextTick(callback);
    this.logger.log("Warning: startMining called when miner was already started");
    return;
  }

  this.is_mining = true;

  if (this.is_mining_on_interval) {
    this.mineOnInterval();
    process.nextTick(callback);
  } else {
    this.processBlocks(callback);
  }
};

StateManager.prototype.stopMining = function(callback) {
  if (this.is_mining) {
    if (this._minerCancellationToken) {
      this._minerCancellationToken.cancelled = true;
      this._minerCancellationToken = null;
    }
    this.is_mining = false;
    clearTimeout(this.mining_interval_timeout);
    this.mining_interval_timeout = null;
  } else {
    this.logger.log("Warning: stopMining called when miner was already stopped");
  }
  callback && process.nextTick(callback);
};

StateManager.prototype.isUnlocked = function(address) {
  return this.unlocked_accounts[address.toLowerCase()] != null;
};

StateManager.prototype.createTransactionWithCorrectNonce = function(tx, from, callback) {
  // account for transactions waiting in the tx queue
  this.blockchain.getQueuedNonce(from, (err, expectedNonce) => {
    if (err) {
      return callback(err);
    }

    const validateNonce = (tx, expectedNonce) => {
      if (tx.validateNonce(expectedNonce)) {
        return Promise.resolve(tx);
      } else {
        const expected = new BN(expectedNonce).toString(10);
        const actual = new BN(tx.nonce).toString(10);
        return Promise.reject(
          new TXRejectedError(
            `the tx doesn't have the correct nonce. account has nonce of: ${expected} tx has nonce of: ${actual}`
          )
        );
      }
    };
    const done = (tx) => callback(null, tx);

    if (tx.isSigned()) {
      validateNonce(tx, expectedNonce)
        .then(done)
        .catch(callback);
    } else {
      const sign = (tx) => {
        // we need to sign transactions if they haven't been signed already
        // but we never sign fake transactions
        if (!tx.isFake() && !tx.isSigned()) {
          const account = this.accounts[from];
          if (account) {
            tx.sign(account.secretKey);
          }
        }
        return Promise.resolve(tx);
      };

      // Validate the tx's nonce and then sign the transaction.
      // By signing this transaction now we ensure all future calls to tx.hash()
      // return the same signed transaction hash. It's sort of an unintuitive
      // quirk of etheremjs-tx that:
      //   tx.hash(includeSignature);
      //   tx.sign(secretKey);
      //   tx.hash(includeSignature);
      // will produce different hashes.
      if (tx.nonce.length === 0) {
        // Since this transaction is unsigned and the nonce was not defined
        // we can go ahead and change the nonce and not worry about
        // invalidating the tx hash the client may be expecting (automatic nonce
        // calculation should have been expected by the client in this case).
        tx.nonce = expectedNonce;
        sign(tx).then(done);
      } else {
        validateNonce(tx, expectedNonce)
          .then(sign)
          .then(done)
          .catch(callback);
      }
    }
  });
};
module.exports = StateManager;
