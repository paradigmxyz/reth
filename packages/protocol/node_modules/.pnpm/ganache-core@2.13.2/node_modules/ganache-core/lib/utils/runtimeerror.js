var inherits = require("util").inherits;
var to = require("./to");
var abi = require("ethereumjs-abi");

inherits(RuntimeError, Error);

// Note: ethereumjs-vm will return an object that has a "results" and "receipts" keys.
// You should pass in the whole object.
function RuntimeError(transactions, vmOutput) {
  // Why not just Error.apply(this, [message])? See
  // https://gist.github.com/justmoon/15511f92e5216fa2624b#anti-patterns
  Error.captureStackTrace(this, this.constructor);
  this.name = this.constructor.name;

  this.results = {};
  this.hashes = [];

  // handles creating this.message
  this.combine(transactions, vmOutput);
}

RuntimeError.prototype.combine = function(transactions, vmOutput) {
  // Can be combined with vmOutput or another RuntimeError.
  if (transactions instanceof RuntimeError) {
    var err = transactions;
    var keys = Object.keys(err.results);

    for (var i = 0; i < keys.length; i++) {
      var key = keys[i];
      this.results[key] = err.results[key];
      Array.prototype.push.apply(this.hashes, err.hashes);
    }
  } else {
    var results = vmOutput.results;

    for (i = 0; i < transactions.length; i++) {
      var tx = transactions[i];
      var result = results[i];

      // 1 means no error, oddly.
      if (result.execResult.exceptionError) {
        var hash = to.hex(tx.hash());
        this.hashes.push(hash);
        var reason;
        var returnData = result.execResult.returnValue;
        if (returnData && returnData.slice(0, 4).toString("hex") === "08c379a0") {
          try {
            reason = abi.rawDecode(["string"], returnData.slice(4))[0];
          } catch (_) {
            // if decoding fails, we'll just leave the reason empty
          }
        }

        const runState = result.execResult.runState;
        this.results[hash] = {
          error: result.execResult.exceptionError.error || result.execResult.exceptionError,
          program_counter: runState && runState.programCounter,
          return: to.hex(returnData),
          reason: reason
        };
      }
    }
  }

  // Once combined, set the message
  if (this.hashes.length === 1) {
    var exceptionResult = this.results[this.hashes[0]];
    var message = "VM Exception while processing transaction: " + exceptionResult.error;
    if (exceptionResult.reason) {
      message += " " + exceptionResult.reason;
    }
    this.message = message;
  } else {
    message = "Multiple VM Exceptions while processing transactions: \n\n";

    for (i = 0; i < this.hashes.length; i++) {
      hash = this.hashes[i];
      exceptionResult = this.results[hash];
      message += hash + ": " + exceptionResult.error;
      if (exceptionResult.reason) {
        message += " " + exceptionResult.reason;
      }
      message += "\n";
    }
    this.message = message;
  }
};

RuntimeError.prototype.count = function() {
  return Object.keys(this.results).length;
};

RuntimeError.fromResults = function(transactions, vmOutput) {
  var err = new RuntimeError(transactions, vmOutput);

  if (err.count() === 0) {
    return null;
  }

  return err;
};

module.exports = RuntimeError;
