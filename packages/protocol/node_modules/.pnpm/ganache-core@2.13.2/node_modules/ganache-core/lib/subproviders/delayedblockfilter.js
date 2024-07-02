// It's unforutnate we have to have this subprovider, but it's because
// we instamine, and web3 isn't written in a way that supports instamining
// (i.e., it sets up the filter after the transaction has been processed).
// This block filter will ensure that each block filter will always see
// the change from the last block to the current block.
//
// Note: An added benefit of this is that it shaves off a signifcant
// amount of time from tests that use web3 and block filters.

var Subprovider = require("web3-provider-engine/subproviders/subprovider.js");
var inherits = require("util").inherits;
var async = require("async");
var to = require("../utils/to");

inherits(DelayedBlockFilter, Subprovider);

module.exports = DelayedBlockFilter;

function DelayedBlockFilter() {
  this.watching = {};
}

DelayedBlockFilter.prototype.handleRequest = function(payload, next, end) {
  if (payload.method === "eth_newBlockFilter") {
    return this.handleNewBlockFilter(payload, next, end);
  }
  if (payload.method === "eth_getFilterChanges") {
    return this.handleGetFilterChanges(payload, next, end);
  }

  next();
};

DelayedBlockFilter.prototype.handleNewBlockFilter = function(payload, next, end) {
  var self = this;

  // Let this filter process and add it to our watch list.
  next(function(err, result, cb) {
    if (err) {
      return cb();
    }
    self.watching[result] = true;
    cb();
  });
};

DelayedBlockFilter.prototype.handleGetFilterChanges = function(payload, next, end) {
  var self = this;
  var filterId = payload.params[0];

  if (!this.watching[filterId]) {
    return next();
  }

  // Get the changes, and then alter the result.
  next(function(err, result, cb) {
    if (err) {
      return cb();
    }

    var currentBlockHash;
    var previousBlockHash;
    var blockNumber;

    async.series(
      [
        function(c) {
          // If we have a result, use it.
          if (result.length !== 0) {
            currentBlockHash = result[0];
            c();
          } else {
            // Otherwise, get the current block number.
            self.emitPayload(
              {
                method: "eth_blockNumber"
              },
              function(err, res) {
                if (err) {
                  return c(err);
                }
                blockNumber = to.number(res.result);
                c();
              }
            );
          }
        },
        function(c) {
          // If we got a block number above, meaning, we didn't get a block hash,
          // skip this step.
          if (blockNumber) {
            return c();
          }

          // If not skipped, then we got a block hash, and we need to get a block number from it.
          self.emitPayload(
            {
              method: "eth_getBlockByHash",
              params: [currentBlockHash, false]
            },
            function(err, res) {
              if (err) {
                return c(err);
              }
              blockNumber = to.number(res.result.number);
              c();
            }
          );
        },
        function(c) {
          // If we're at block 0, return no changes. See final function below.
          blockNumber = to.number(blockNumber);
          if (blockNumber === 0) {
            previousBlockHash = undefined;
            return c();
          }

          // If at this point, we do have a block number, so let's subtract one
          // from it and get the block hash of the block before it.
          blockNumber = blockNumber - 1;
          self.emitPayload(
            {
              method: "eth_getBlockByNumber",
              params: [blockNumber, false]
            },
            function(err, res) {
              if (err) {
                return c(err);
              }
              previousBlockHash = res.result.hash;
              c();
            }
          );
        }
      ],
      function(err) {
        if (err) {
          // Unfortunately the subprovider code doesn't let us return an error
          // through the callback cb(). So we'll just ignore it.... (famous last words).
        }

        // If we got the previous block, use it. Otherwise do nothing.
        // Then stop watching because we only want on getFilterChanges to react this way.
        if (previousBlockHash) {
          result[0] = previousBlockHash;
        }

        delete self.watching[filterId];
        cb();
      }
    );
  });
};
