var to = require("./utils/to.js");
var Account = require("ethereumjs-account").default;
var Block = require("ethereumjs-block");
var Log = require("./utils/log");
var Receipt = require("./utils/receipt");
var VM = require("ethereumjs-vm").default;
var Common = require("ethereumjs-common").default;
var RuntimeError = require("./utils/runtimeerror");
var Trie = require("merkle-patricia-tree");
var utils = require("ethereumjs-util");
var async = require("async");
var Heap = require("heap");
var Database = require("./database");
var EventEmitter = require("events");
var estimateGas = require("./utils/gas/estimateGas");
var _ = require("lodash");
var promisify = require("util").promisify;
const BN = utils.BN;

function BlockchainDouble(options) {
  var self = this;
  EventEmitter.apply(self);

  this.options = options = this._applyDefaultOptions(options || {});

  this.logger = options.logger || console;

  this.data = new Database(options);

  if (options.trie != null && options.db_path != null) {
    throw new Error("Can't initialize a TestRPC with a db and a custom trie.");
  }

  this.pending_transactions = [];

  // updated periodically to keep up with the times
  this.blockGasLimit = options.gasLimit;
  this.defaultTransactionGasLimit = options.defaultTransactionGasLimit;
  this.timeAdjustment = 0;
}

const defaultOptions = {
  gasLimit: "0x6691b7",
  defaultTransactionGasLimit: "0x15f90",
  time: null,
  debug: false,
  hardfork: "muirGlacier",
  allowUnlimitedContractSize: false
};

// inheritence w/ prototype chaining
BlockchainDouble.prototype = Object.create(EventEmitter.prototype);
BlockchainDouble.prototype.constructor = BlockchainDouble;

BlockchainDouble.prototype._applyDefaultOptions = function(options) {
  // We want this function to mutate the options object so that we can report
  // our settings back to our consumer application (e.g., ganache)
  return _.merge(options, defaultOptions, Object.assign({}, options));
};

BlockchainDouble.prototype.initialize = function(accounts, callback) {
  var self = this;

  this.data.initialize(function(err) {
    if (err) {
      return callback(err);
    }

    self.latestBlock(function(err, block) {
      if (err) {
        return callback(err);
      }

      var options = self.options;

      var root = null;

      if (block) {
        root = block.header.stateRoot;
      }

      // I haven't yet found a good way to do this. Getting the trie from the
      // forked blockchain without going through the other setup is a little gross.
      self.stateTrie = self.createStateTrie(self.data.trie_db, root);

      self.vm = options.vm || self.createVMFromStateTrie(self.stateTrie, true);

      if (options.time) {
        self.setTime(options.time);
      }

      // If we already have a block, then that means there's an existing chain.
      // Don't create a genesis block.
      if (block) {
        self.emit("block", block);
        return callback();
      }

      self.createGenesisBlock(function(err, block) {
        if (err) {
          return callback(err);
        }

        accounts = accounts || [];

        self.vm.stateManager.checkpoint(() => {
          async.eachSeries(
            accounts,
            function(accountData, finished) {
              self.vm.stateManager.putAccount(utils.toBuffer(accountData.address), accountData.account, finished);
            },
            function(err) {
              if (err) {
                return callback(err);
              }

              self.vm.stateManager.commit(() => {
                // Create first block
                self.putBlock(block, [], [], callback);
              });
            }
          );
        });
      });
    });
  });
};

BlockchainDouble.prototype.createVMFromStateTrie = function(state, activatePrecompiles) {
  const self = this;
  const common = Common.forCustomChain(
    "mainnet", // TODO needs to match chain id
    {
      name: "ganache",
      networkId: self.options.network_id || self.forkVersion,
      chainId: self.options._chainId,
      comment: "Local test network",
      bootstrapNodes: []
    },
    self.options.hardfork
  );

  const vm = new VM({
    state: state,
    common,
    blockchain: {
      // EthereumJS VM needs a blockchain object in order to get block information.
      // When calling getBlock() it will pass a number that's of a Buffer type.
      // Unfortunately, it uses a 64-character buffer (when converted to hex) to
      // represent block numbers as well as block hashes. Since it's very unlikely
      // any block number will get higher than the maximum safe Javascript integer,
      // we can convert this buffer to a number ahead of time before calling our
      // own getBlock(). If the conversion succeeds, we have a block number.
      // If it doesn't, we have a block hash. (Note: Our implementation accepts both.)
      getBlock: function(number, done) {
        try {
          number = to.number(number);
        } catch (e) {
          // Do nothing; must be a block hash.
        }

        self.getBlock(number, done);
      }
    },
    activatePrecompiles: activatePrecompiles || false,
    allowUnlimitedContractSize: self.options.allowUnlimitedContractSize
  });

  if (self.options.debug === true) {
    // log executed opcodes, including args as hex
    vm.on("step", function(info) {
      var name = info.opcode.name;
      var argsNum = info.opcode.in;
      if (argsNum) {
        var args = info.stack
          .slice(-argsNum)
          .map((arg) => to.hex(arg))
          .join(" ");

        self.logger.log(`${name} ${args}`);
      } else {
        self.logger.log(name);
      }
    });
  }

  return vm;
};

BlockchainDouble.prototype.createStateTrie = function(db, root) {
  return new Trie(db, root);
};

// Overrideable so other implementations (forking) can edit it.
BlockchainDouble.prototype.createGenesisBlock = function(callback) {
  this.createBlock(callback);
};

BlockchainDouble.prototype.latestBlock = function(callback) {
  this.data.blocks.last(function(err, last) {
    if (err) {
      return callback(err);
    }
    callback(null, last);
  });
};

// number accepts number (integer, hex) or tag (e.g., "latest")
BlockchainDouble.prototype.getEffectiveBlockNumber = function(number, callback) {
  if (typeof number !== "string") {
    number = to.hex(number);
  }

  // If we have a hex number
  if (number.indexOf("0x") >= 0) {
    return callback(null, to.number(number));
  } else {
    if (number === "latest" || number === "pending") {
      return this.getHeight(callback);
    } else if (number === "earliest") {
      return callback(null, 0);
    }
  }
};

// number accepts number (integer, hex), tag (e.g., "latest") or block hash
// This function is used by ethereumjs-vm
BlockchainDouble.prototype.getBlock = function(number, callback) {
  var self = this;

  if (typeof number !== "string") {
    number = to.hex(number);
  }

  // If we have a hex number or a block hash
  if (number.indexOf("0x") >= 0) {
    var hash = number;

    // block hash
    if (hash.length > 40) {
      this.data.blockHashes.get(to.hex(hash), function(err, blockIndex) {
        if (err) {
          return callback(err);
        }
        return self.data.blocks.get(blockIndex, callback);
      });
    } else {
      // Block number
      return this.data.blocks.get(to.number(hash), callback);
    }
  } else {
    if (number === "latest" || number === "pending") {
      return this.latestBlock(callback);
    } else if (number === "earliest") {
      return this.data.blocks.first(callback);
    } else {
      process.nextTick(callback, new Error("Invalid `blockNumber`: \"" + number + "\""));
    }
  }
};

BlockchainDouble.prototype.putBlock = function(block, logs, receipts, callback) {
  var self = this;

  // Lock in the state root for this block.
  block.header.stateRoot = this.stateTrie.root;

  this.data.blocks.length(function(err, length) {
    if (err) {
      return callback(err);
    }

    var requests = [
      self.data.blocks.push.bind(self.data.blocks, block),
      self.data.blockLogs.push.bind(self.data.blockLogs, logs),
      self.data.blockHashes.set.bind(self.data.blockHashes, to.hex(block.hash()), length)
    ];

    block.transactions.forEach(function(tx, index) {
      var txHash = to.hex(tx.hash());
      requests.push(
        self.data.transactions.set.bind(self.data.transactions, txHash, tx),
        self.data.transactionReceipts.set.bind(self.data.transactionReceipts, txHash, receipts[index])
      );
    });

    async.parallel(requests, (err, result) => {
      if (!err) {
        self.emit("block", block);
      }
      callback(err, result);
    });
  });
};

BlockchainDouble.prototype.popBlock = function(callback) {
  var self = this;

  this.data.blocks.last(function(err, block) {
    if (err) {
      return callback(err);
    }
    if (block == null) {
      return callback(null, null);
    }

    var requests = [];
    var blockHash = to.hex(block.hash());

    block.transactions.forEach(function(tx) {
      var txHash = to.hex(tx.hash());

      requests.push(
        self.data.transactions.del.bind(self.data.transactions, txHash),
        self.data.transactionReceipts.del.bind(self.data.transactionReceipts, txHash)
      );
    });

    requests.push(
      self.data.blockLogs.pop.bind(self.data.blockLogs),
      self.data.blockHashes.del.bind(self.data.blockHashes, blockHash),
      self.data.blocks.pop.bind(self.data.blocks) // Do this one last in case anything relies on it.
    );

    async.series(requests, function(err) {
      if (err) {
        return callback(err);
      }

      // Set the root to the last available, which will "roll back" to the previous
      // moment in time. Note that all the old data is still in the db, but it's now just junk data.
      self.data.blocks.last(function(err, newLastBlock) {
        if (err) {
          return callback(err);
        }
        // using setStateRoot because in the future it will automatically take care
        // of clearing the cache for us.
        // note setStateRoot checks for checkpoints, and if there are any, it will fail.
        // At time of writing this comment, the only time there could be a checkpoint
        // is in the middle of a vm.runBlock call. Once asyncRequestProcessing is reenabled
        // this will likely cause problems.
        self.vm.stateManager.setStateRoot(newLastBlock.header.stateRoot, function(err) {
          // Remember: Return block we popped off.
          callback(err, block);
        });
      });
    });
  });
};

BlockchainDouble.prototype.clearPendingTransactions = function() {
  this.pending_transactions = [];
};

/**
 * createBlock
 *
 * Create a new block, where the parent's block is either the latest block
 * on the chain or the parent block passed in.
 *
 * @param  {Block}   parent   The block meant to be the parent block (optional)
 * @param  {Function} callback Callback function called after block is created
 * @return Block              The block created.
 */
BlockchainDouble.prototype.createBlock = function(parent, emulateParent, callback) {
  var self = this;

  if (typeof parent === "function") {
    callback = parent;
    parent = null;
    emulateParent = false;
  } else if (typeof emulateParent === "function") {
    callback = emulateParent;
    emulateParent = false;
  }

  var block = new Block();

  function getParent(callback) {
    if (parent) {
      return callback(null, parent);
    } else {
      self.latestBlock(callback);
    }
  }

  getParent(function(err, parent) {
    if (err) {
      return callback(err);
    }

    block.header.gasLimit = self.blockGasLimit;

    if (parent != null && emulateParent) {
      block.header.number = parent.header.number;
      block.header.timestamp = parent.header.timestamp;
      block.header.parentHash = parent.header.parentHash;
    } else {
      var parentNumber = parent != null ? to.number(parent.header.number) : -1;

      // Ensure we have the right block number for the VM.
      block.header.number = to.hex(parentNumber + 1);

      // Set the timestamp before processing txs
      block.header.timestamp = to.hex(self.currentTime());

      if (parent != null) {
        block.header.parentHash = to.hex(parent.hash());
      }
    }

    callback(null, block);
  });
};

BlockchainDouble.prototype.getQueuedNonce = function(address, callback) {
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
  this.stateTrie.get(addressBuffer, function(err, val) {
    if (err) {
      return callback(err);
    }

    var account = new Account(val);
    // nonces are initiallized as an empty buffer, which isn't what we want.
    callback(null, account.nonce.length === 0 ? Buffer.from([0]) : account.nonce);
  });
};

BlockchainDouble.prototype.queueTransaction = function(tx) {
  this.pending_transactions.push(tx);
};

BlockchainDouble.prototype.sortByPriceAndNonce = function() {
  // Sorts transactions like I believe geth does.
  // See the description of 'SortByPriceAndNonce' at
  // https://github.com/ethereum/go-ethereum/blob/290e851f57f5d27a1d5f0f7ad784c836e017c337/core/types/transaction.go
  var self = this;
  var sortedByNonce = {};

  self.pending_transactions.forEach((tx) => {
    const from = tx.from.toString("hex");
    const arr = sortedByNonce[from];
    if (arr) {
      arr.push(tx);
    } else {
      sortedByNonce[from] = [tx];
    }
  });

  var priceSort = function(a, b) {
    return parseInt(to.hex(b.gasPrice), 16) - parseInt(to.hex(a.gasPrice), 16);
  };
  var nonceSort = function(a, b) {
    return parseInt(to.hex(a.nonce), 16) - parseInt(to.hex(b.nonce), 16);
  };

  // Now sort each address by nonce
  Object.keys(sortedByNonce).forEach((address) => {
    sortedByNonce[address].sort(nonceSort);
  });

  // Initialise a heap, sorted by price, for the head transaction from each account.
  var heap = new Heap(priceSort);
  Object.keys(sortedByNonce).forEach((address) => {
    heap.push(sortedByNonce[address].shift());
  });

  // Now reorder our transactions. Compare the next transactions from each account, and choose
  // the one with the highest gas price.
  const sortedTransactions = [];
  while (heap.size() > 0) {
    const best = heap.pop();
    const address = best.from.toString("hex");
    if (sortedByNonce[address].length > 0) {
      // Push on the next transaction from this account
      heap.push(sortedByNonce[address].shift());
    }
    sortedTransactions.push(best);
  }
  self.pending_transactions = sortedTransactions;
};

BlockchainDouble.prototype.getReadyCall = function(tx, emulateParent, blockNumber, callback) {
  const readyCall = (tx, err, parentBlock) => {
    if (err) {
      return callback(err);
    }
    // create a fake block with this fake transaction
    this.createBlock(parentBlock, emulateParent, (err, newBlock) => {
      if (err) {
        return callback(err);
      }

      newBlock.transactions.push(tx);

      // gas estimates and eth_calls aren't subject to block gas limits
      newBlock.header.gasLimit = tx.gasLimit;

      const runArgs = {
        tx: tx,
        block: newBlock,
        skipBalance: true,
        skipNonce: true
      };

      callback(null, parentBlock.header.stateRoot, runArgs);
    });
  };
  // Delegate block selection
  if (blockNumber === "latest") {
    this.latestBlock(readyCall.bind(null, tx));
  } else {
    this.getBlock(blockNumber, readyCall.bind(null, tx));
  }
};

BlockchainDouble.prototype.readyCall = function(tx, emulateParent, blockNumber, callback) {
  this.getReadyCall(tx, emulateParent, blockNumber, (err, stateRoot, runArgs) => {
    if (err) {
      callback(err);
      return;
    }
    const stateTrie = this.createStateTrie(this.data.trie_db, stateRoot, { persist: false });
    const vm = this.createVMFromStateTrie(stateTrie);
    callback(null, vm, runArgs);
  });
};

BlockchainDouble.prototype.processCall = function(tx, blockNumber, callback) {
  this.readyCall(tx, true, blockNumber, async(err, vm, runArgs) => {
    if (err) {
      callback(err);
      return;
    }

    const result = await vm.runTx(runArgs).catch((vmerr) => ({ vmerr }));
    let vmerr = result.vmerr;
    // This is a check that has been in there for awhile. I'm unsure if it's required, but it can't hurt.
    if (vmerr && vmerr instanceof Error === false) {
      vmerr = new Error("VM error: " + vmerr);
    }

    // If we're given an error back directly, it's worse than a runtime error. Expose it and get out.
    if (vmerr) {
      return callback(vmerr, err);
    }

    // If no error, check for a runtime error. This can return null if no runtime error.
    vmerr = RuntimeError.fromResults([tx], { results: [result] });

    callback(vmerr, result);
  });
};

BlockchainDouble.prototype.estimateGas = function(tx, blockNumber, callback) {
  this.getReadyCall(tx, false, blockNumber, (err, stateRoot, runArgs) => {
    if (err) {
      callback(err);
      return;
    }
    const generateVM = () => {
      const stateTrie = this.createStateTrie(this.data.trie_db, stateRoot);
      return this.createVMFromStateTrie(stateTrie);
    };
    estimateGas(generateVM, runArgs, callback);
  });
};

/**
 * processBlock
 *
 * Process the passed in block and included transactions
 *
 * @param  {VM} vm             the vm to use when running the block
 * @param  {Block} block       block to process
 * @param  {Boolean} commit    Whether or not changes should be committed to the state
 * trie and the block appended to the end of the chain.
 * @param  {Function} callback Callback function when transaction processing is completed.
 * @return [type]              [description]
 */
BlockchainDouble.prototype.processBlock = async function(vm, block, commit, callback) {
  var self = this;

  if (typeof commit === "function") {
    callback = commit;
    commit = true;
  }

  const results = await vm
    .runBlock({
      block: block,
      generate: true,
      skipBlockValidation: true
    })
    .catch((vmerr) => ({ vmerr }));
  let vmerr = results.vmerr;
  // This is a check that has been in there for awhile. I'm unsure if it's required, but it can't hurt.
  if (vmerr && vmerr instanceof Error === false) {
    vmerr = new Error("VM error: " + vmerr);
  }

  // If we're given an error back directly, it's worse than a runtime error. Expose it and get out.
  if (vmerr) {
    callback(vmerr);
    return;
  }
  // If no error, check for a runtime error. This can return null if no runtime error.
  vmerr = RuntimeError.fromResults(block.transactions, results);

  // Note, even if we have an error, some transactions may still have succeeded.
  // Process their logs if so, returning the error at the end.

  var logs = [];
  var receipts = [];

  var totalBlockGasUsage = 0;

  results.results.forEach(function(result) {
    totalBlockGasUsage += to.number(result.gasUsed);
  });

  block.header.gasUsed = utils.toBuffer(to.hex(totalBlockGasUsage));

  const txTrie = new Trie();
  const rcptTrie = new Trie();
  const promises = [];
  const putInTrie = (trie, key, val) => promisify(trie.put.bind(trie))(key, val);

  for (var v = 0; v < results.receipts.length; v++) {
    var result = results.results[v];
    var receipt = results.receipts[v];
    var tx = block.transactions[v];
    var txHash = tx.hash();
    var txLogs = [];

    // Only process the transaction's logs if it didn't error.
    if (result.execResult.exceptionError === undefined) {
      for (var i = 0; i < receipt.logs.length; i++) {
        var receiptLog = receipt.logs[i];
        var address = to.hex(receiptLog[0]);
        var topics = [];

        for (var j = 0; j < receiptLog[1].length; j++) {
          topics.push(to.hex(receiptLog[1][j]));
        }

        var data = to.hex(receiptLog[2]);

        var log = new Log({
          logIndex: to.hex(i),
          transactionIndex: to.hex(v),
          transactionHash: txHash,
          block: block,
          address: address,
          data: data,
          topics: topics,
          type: "mined"
        });

        logs.push(log);
        txLogs.push(log);
      }
    }

    const rcpt = new Receipt(
      tx,
      block,
      txLogs,
      result.gasUsed.toArrayLike(Buffer),
      receipt.gasUsed,
      result.createdAddress,
      receipt.status,
      to.hex(receipt.bitvector)
    );
    receipts.push(rcpt);

    const rawReceipt = [receipt.status, receipt.gasUsed, receipt.bitvector, receipt.logs];
    const rcptBuffer = utils.rlp.encode(rawReceipt);
    const key = utils.rlp.encode(v);
    promises.push(putInTrie(txTrie, key, tx.serialize()));
    promises.push(putInTrie(rcptTrie, key, rcptBuffer));
  }
  await Promise.all(promises);

  block.header.transactionsTrie = utils.toBuffer(txTrie.root);
  block.header.receiptTrie = utils.toBuffer(rcptTrie.root);

  if (commit) {
    // Put that block on the end of the chain
    self.putBlock(block, logs, receipts, done);
  } else {
    done();
  }

  function done(e) {
    if (e) {
      return callback(e);
    }
    // Note we return the vm err here too, if it exists.
    callback(vmerr, block.transactions, results);
  }
};

/**
 * processNextBlock
 *
 * Process the next block like a normal blockchain, pulling from the list of
 * pending transactions.
 *
 * @param  {number} timestamp at which the block is mined
 * @param  {Function} callback Callback when transaction processing is finished.
 * @return [type]              [description]
 */
BlockchainDouble.prototype.processNextBlock = function(timestamp, callback) {
  var self = this;

  if (typeof timestamp === "function") {
    callback = timestamp;
    timestamp = undefined;
  }

  self.sortByPriceAndNonce();

  // Grab only the transactions that can fit within the block
  var currentTransactions = [];
  var totalGasLimit = 0;
  var maxGasLimit = to.number(self.blockGasLimit);

  while (self.pending_transactions.length > 0) {
    var tx = self.pending_transactions[0];
    var gasLimit = to.number(tx.gasLimit);

    if (totalGasLimit + gasLimit <= maxGasLimit) {
      totalGasLimit += gasLimit;
      self.pending_transactions.shift();
      currentTransactions.push(tx);
    } else {
      // Next one won't fit. Break.
      break;
    }
  }

  // Remember, we ensured transactions had a valid gas limit when they were queued (in the state manager).
  // If we run into a case where we can't process any because one is higher than the gas limit,
  // then it's a serious issue. This should never happen, but let's check anyway.
  if (currentTransactions.length === 0 && self.pending_transactions.length > 0) {
    // Error like geth.
    var error = "Unexpected error condition: next transaction exceeds block gas limit";
    return callback(error);
  }

  // Create a new block meant to be the end of the chain
  this.createBlock(function(err, block) {
    if (err) {
      return callback(err);
    }

    // Overwrite block timestamp
    if (timestamp) {
      self.data.blocks.last(function(err, last) {
        if (err) {
          // it is safe to ignore this error as we only use the result
          // to log a warning to the console.
          return;
        }
        if (last && to.number(last.header.timestamp) > timestamp) {
          self.logger.log(
            "Waring: Setting the block timestamp (" + timestamp + ") that is earlier than the parent block one."
          );
        }
      });
      block.header.timestamp = to.hex(timestamp);
      self.setTime(new Date(timestamp * 1000));
    }
    // Add transactions to the block.
    Array.prototype.push.apply(block.transactions, currentTransactions);

    self.processBlock(self.vm, block, true, callback);
  });
};

/**
 * processTransactionTrace
 *
 * Run a previously-run transaction in the same state in which it occurred at the time it was run.
 * This will return the vm-level trace output for debugging purposes.
 *
 * Strategy:
 *
 *  1. Find block where transaction occurred
 *  2. Set state root of that block
 *  3. Rerun every transaction in that block prior to and including the requested transaction
 *  4. Send trace results back.
 *
 * @param  {[type]}   tx       [description]
 * @param  {Function} callback [description]
 * @return [type]              [description]
 */
BlockchainDouble.prototype.processTransactionTrace = async function(hash, params, callback) {
  const self = this;
  const targetHash = to.hex(hash);
  let txHashCurrentlyProcessing = "";
  let txCurrentlyProcessing = null;
  let vm;

  const storageStack = {
    currentDepth: -1,
    stack: []
  };

  const returnVal = {
    gas: 0,
    returnValue: "",
    structLogs: []
  };

  function stepListener(event, next) {
    // See these docs:
    // https://github.com/ethereum/go-ethereum/wiki/Management-APIs

    const gasLeft = to.number(event.gasLeft);
    const totalGasUsedAfterThisStep = to.number(txCurrentlyProcessing.gasLimit) - gasLeft;
    const gasUsedPreviousStep = totalGasUsedAfterThisStep - returnVal.gas;
    returnVal.gas += gasUsedPreviousStep;

    let memory = null;
    if (!params.disableMemory) {
      // Get memory and break it up into 32-byte words.
      // Note we may possibly have to pad the final word.
      memory = Buffer.from(event.memory).toString("hex");
      memory = memory.match(/.{1,64}/g) || [];

      if (memory.length > 0) {
        const lastItem = memory[memory.length - 1];
        if (lastItem.length < 64) {
          memory[memory.length - 1] = lastItem + new Array(64 - lastItem.length + 1).join("0");
        }
      }
    }

    let stack = null;
    if (!params.disableStack) {
      stack = event.stack.map((item) => {
        return to.rpcDataHexString(item, 64).replace("0x", ""); // non-0x prefixed.
      });
    }
    let structLog = {
      depth: event.depth,
      error: "",
      gas: gasLeft,
      gasCost: 0,
      memory,
      op: event.opcode.name,
      pc: event.pc,
      stack,
      storage: null
    };

    // The gas difference calculated for each step is indicative of gas consumed in
    // the previous step. Gas consumption in the final step will always be zero.
    if (returnVal.structLogs.length) {
      returnVal.structLogs[returnVal.structLogs.length - 1].gasCost = gasUsedPreviousStep;
    }

    if (params.disableStorage) {
      returnVal.structLogs.push(structLog);
      next();
    } else {
      structLog = self.processStorageTrace(structLog, storageStack, event, vm, function(err, structLog) {
        if (err) {
          return next(err);
        }
        returnVal.structLogs.push(structLog);
        next();
      });
    }
  }

  // #1 - get block via transaction receipt
  this.getTransactionReceipt(targetHash, function(err, receipt) {
    if (err) {
      return callback(err);
    }

    if (!receipt) {
      return callback(new Error("Unknown transaction " + targetHash));
    }

    const targetBlock = receipt.block;

    // Get the parent of the target block
    self.getBlock(targetBlock.header.parentHash, function(err, parent) {
      if (err) {
        return callback(err);
      }

      // #2 - Set state root of original block
      var stateTrie = self.createStateTrie(self.data.trie_db, parent.header.stateRoot, {
        // when forking we need to make sure we also copy over the forkBlockNumber,
        // otherwise some operations will request data from the main chain at the
        // latest block
        forkBlockNumber: to.number(parent.header.number)
      });
      vm = self.createVMFromStateTrie(stateTrie);

      // Prepare the "next" block with necessary transactions
      self.createBlock(parent, false, function(err, block) {
        if (err) {
          return callback(err);
        }

        // make sure we use the same timestamp as the target block
        block.header.timestamp = targetBlock.header.timestamp;

        for (var i = 0; i < targetBlock.transactions.length; i++) {
          var tx = targetBlock.transactions[i];
          block.transactions.push(tx);

          // After including the target transaction, that's all we need to do.
          if (to.hex(tx.hash()) === targetHash) {
            break;
          }
        }

        function beforeTxListener(tx) {
          txCurrentlyProcessing = tx;
          txHashCurrentlyProcessing = to.hex(tx.hash());
          if (txHashCurrentlyProcessing === targetHash) {
            vm.on("step", stepListener);
          }
        }

        // afterTxListener cleans up everything.
        function afterTxListener() {
          if (txHashCurrentlyProcessing === targetHash) {
            removeListeners();
          }
        }

        function removeListeners() {
          vm.removeListener("step", stepListener);
          vm.removeListener("beforeTx", beforeTxListener);
          vm.removeListener("afterTx", afterTxListener);
        }

        // Listen to beforeTx and afterTx so we know when our target transaction
        // is processing. These events will add the event listener for getting the trace data.
        vm.on("beforeTx", beforeTxListener);
        vm.on("afterTx", afterTxListener);

        // Don't even let the vm try to flush the block's _cache to the stateTrie.
        // When forking some of the data that the traced function may request will
        // exist only on the main chain. Because we pretty much lie to the VM by
        // telling it we DO have data in our Trie, when we really don't, it gets
        // lost during the commit phase when it traverses the "borrowed" data's
        // trie (as it may not have a valid root). Because this is a trace, and we
        // don't need to commit the data, duck punching the `flush` method (the
        // simplest method I could find) is fine.
        // Remove this and you may see the infamous
        // `Uncaught TypeError: Cannot read property 'pop' of undefined` error!
        vm.stateManager._cache.flush = (cb) => cb();

        // #3 - Process the block without committing the data.
        self.processBlock(vm, block, false, function(err) {
          // Ignore runtime errors, or else erroneous transactions can't be traced.
          if (err && err.message.indexOf("VM Exception") === 0) {
            err = null;
          }

          // Just to be safe
          removeListeners();

          // #4 - send state results back
          callback(err, returnVal);
        });
      });
    });
  });
};

BlockchainDouble.prototype.processStorageTrace = function(structLog, storageStack, event, vm, callback) {
  var name = event.opcode.name;

  if (storageStack.currentDepth > event.depth) {
    storageStack.stack.pop();
  }
  if (storageStack.currentDepth < event.depth) {
    storageStack.stack.push({});
  }

  storageStack.currentDepth = event.depth;

  var key;
  var value;
  switch (name) {
    case "SSTORE":
      {
        const stack = event.stack;
        const stackLength = stack.length;
        key = to.rpcDataHexString(stack[stackLength - 1], 64).replace("0x", "");
        value = to.rpcDataHexString(stack[stackLength - 2], 64).replace("0x", "");
        // use Object.assign to prevent future steps from overwriting this step's storage values
        structLog.storage = Object.assign({}, storageStack.stack[storageStack.currentDepth]);

        callback(null, structLog);
        // assign after callback because this storage change actually takes
        // effect _after_ this opcode executes
        storageStack.stack[storageStack.currentDepth][key] = value;
      }
      break;
    case "SLOAD":
      {
        const stack = event.stack;
        // this one's more fun, we need to get the value the contract is loading from current storage
        key = to.rpcDataHexString(stack[stack.length - 1], 64).replace("0x", "");

        vm.stateManager.getContractStorage(event.address, Buffer.from(key, "hex"), function(err, result) {
          if (err) {
            return callback(err);
          }

          value = to.rpcDataHexString(result, 64).replace("0x", "");
          storageStack.stack[storageStack.currentDepth][key] = value;
          // use Object.assign to prevent future steps from overwriting this step's storage values
          structLog.storage = Object.assign({}, storageStack.stack[storageStack.currentDepth]);
          callback(null, structLog);
        });
      }
      break;
    default:
      // use Object.assign to prevent future steps from overwriting this step's storage values
      structLog.storage = Object.assign({}, storageStack.stack[storageStack.currentDepth]);
      callback(null, structLog);
  }
};

BlockchainDouble.prototype.getAccount = function(address, number, callback) {
  var self = this;

  this.getBlock(number, function(err, block) {
    if (err) {
      return callback(err);
    }

    var trie = self.stateTrie;

    // Manipulate the state root in place to maintain checkpoints
    var currentStateRoot = trie.root;
    self.stateTrie.root = block.header.stateRoot;

    trie.get(utils.toBuffer(address), function(err, data) {
      // Finally, put the stateRoot back for good
      trie.root = currentStateRoot;

      if (err) {
        return callback(err);
      }

      var account = new Account(data);

      callback(null, account);
    });
  });
};

BlockchainDouble.prototype.getNonce = function(address, number, callback) {
  this.getAccount(address, number, function(err, account) {
    if (err) {
      return callback(err);
    }
    callback(null, account.nonce);
  });
};

BlockchainDouble.prototype.getBalance = function(address, number, callback) {
  this.getAccount(address, number, function(err, account) {
    if (err) {
      return callback(err);
    }

    callback(null, account.balance);
  });
};

// Note! Storage values are returned RLP encoded!
BlockchainDouble.prototype.getStorage = function(address, position, number, callback) {
  var self = this;

  this.getBlock(number, function(err, block) {
    if (err) {
      return callback(err);
    }

    var trie = self.stateTrie;

    // Manipulate the state root in place to maintain checkpoints
    var currentStateRoot = trie.root;
    self.stateTrie.root = block.header.stateRoot;

    trie.get(utils.toBuffer(address), function(err, data) {
      if (err != null) {
        // Put the stateRoot back if there's an error
        trie.root = currentStateRoot;
        return callback(err);
      }

      var account = new Account(data);

      trie.root = account.stateRoot;

      trie.get(utils.setLengthLeft(utils.toBuffer(position), 32), function(err, value) {
        // Finally, put the stateRoot back for good
        trie.root = currentStateRoot;

        if (err != null) {
          return callback(err);
        }

        callback(null, value);
      });
    });
  });
};

BlockchainDouble.prototype.getCode = function(address, number, callback) {
  var self = this;

  this.getBlock(number, function(err, block) {
    if (err) {
      return callback(err);
    }

    var trie = self.stateTrie;

    // Manipulate the state root in place to maintain checkpoints
    var currentStateRoot = trie.root;
    self.stateTrie.root = block.header.stateRoot;

    trie.get(utils.toBuffer(address), function(err, data) {
      if (err != null) {
        // Put the stateRoot back if there's an error
        trie.root = currentStateRoot;
        return callback(err);
      }

      var account = new Account(data);

      account.getCode(trie, function(err, code) {
        // Finally, put the stateRoot back for good
        trie.root = currentStateRoot;

        if (err) {
          return callback(err);
        }

        callback(null, code);
      });
    });
  });
};

BlockchainDouble.prototype.getTransaction = function(hash, callback) {
  hash = to.hex(hash);

  this.data.transactions.get(hash, function(err, tx) {
    if (err) {
      if (err.notFound) {
        return callback(null, null);
      } else {
        return callback(err);
      }
    }
    callback(null, tx);
  });
};

BlockchainDouble.prototype.getTransactionReceipt = function(hash, callback) {
  hash = to.hex(hash);
  const pendingTxs = this.pending_transactions;

  for (var i = 0; i < pendingTxs.length; i++) {
    const pendingTxHash = to.hex(pendingTxs[i].hash());
    if (hash === pendingTxHash) {
      return callback(null, { tx: pendingTxs[i] });
    }
  }

  this.data.transactionReceipts.get(hash, function(err, receipt) {
    if (err) {
      if (err.notFound) {
        return callback(null, null);
      } else {
        return callback(err);
      }
    }

    callback(err, receipt);
  });
};

BlockchainDouble.prototype.getBlockLogs = function(number, callback) {
  var self = this;
  this.getEffectiveBlockNumber(number, function(err, effective) {
    if (err) {
      return callback(err);
    }
    self.data.blockLogs.get(effective, callback);
  });
};

BlockchainDouble.prototype.getHeight = function(callback) {
  this.data.blocks.length(function(err, length) {
    if (err) {
      return callback(err);
    }
    callback(null, length - 1);
  });
};

BlockchainDouble.prototype.currentTime = function() {
  return ((new Date().getTime() / 1000) | 0) + this.timeAdjustment;
};

BlockchainDouble.prototype.increaseTime = function(seconds) {
  if (seconds < 0) {
    seconds = 0;
  }
  this.timeAdjustment += seconds;
  return this.timeAdjustment;
};

BlockchainDouble.prototype.setTime = function(date) {
  var now = (new Date().getTime() / 1000) | 0;
  var start = (date.getTime() / 1000) | 0;
  this.timeAdjustment = start - now;
};

BlockchainDouble.prototype.close = function(callback) {
  this.data.close(callback);
};

module.exports = BlockchainDouble;
