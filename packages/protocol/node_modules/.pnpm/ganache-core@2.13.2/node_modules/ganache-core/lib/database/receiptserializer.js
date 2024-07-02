var Receipt = require("../utils/receipt");
var async = require("async");

function ReceiptSerializer(database) {
  this.database = database;
}

ReceiptSerializer.prototype.encode = function(receipt, done) {
  done(null, receipt.toJSON());
};

ReceiptSerializer.prototype.decode = function(json, done) {
  var self = this;
  // Make sure we can handle mixed/upper-case transaction hashes
  // it doesn't seem possible to record a transaction hash that isn't
  // already lower case, as that's the way ganache generates them, however
  // I don't think it will hurt anything to normalize here anyway.
  // If you can figure out how to test this please feel free to add a test!
  var txHash = json.transactionHash.toLowerCase();

  this.database.transactions.get(json.transactionHash, function(err, tx) {
    if (err) {
      return done(err);
    }

    self.database.blockHashes.get(json.blockHash, function(err, blockIndex) {
      if (err) {
        return done(err);
      }

      async.parallel(
        {
          block: self.database.blocks.get.bind(self.database.blocks, blockIndex),
          logs: self.database.blockLogs.get.bind(self.database.blockLogs, blockIndex)
        },
        function(err, result) {
          if (err) {
            return done(err);
          }

          done(
            null,
            new Receipt(
              tx,
              result.block,
              result.logs.filter((log) => log.transactionHash.toLowerCase() === txHash),
              json.gasUsed,
              json.cumulativeGasUsed,
              json.contractAddress,
              json.status,
              json.logsBloom
            )
          );
        }
      );
    });
  });
};

module.exports = ReceiptSerializer;
