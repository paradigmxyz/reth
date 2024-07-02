var Log = require("../utils/log");
var async = require("async");

function BlockLogsSerializer(database) {
  this.database = database;
}

BlockLogsSerializer.prototype.encode = function(logs, done) {
  logs = logs.map(function(log) {
    return log.toJSON();
  });

  done(null, logs);
};

BlockLogsSerializer.prototype.decode = function(json, done) {
  var self = this;

  if (json.length === 0) {
    return done(null, []);
  }

  this.database.blockHashes.get(json[0].blockHash, function(err, blockIndex) {
    if (err) {
      return done(err);
    }

    self.database.blocks.get(blockIndex, function(err, block) {
      if (err) {
        return done(err);
      }

      async.map(
        json,
        function(log, finished) {
          finished(
            null,
            new Log({
              block: block,
              logIndex: log.logIndex,
              transactionIndex: log.transactionIndex,
              transactionHash: log.transactionHash,
              address: log.address,
              data: log.data,
              topics: log.topics,
              type: log.type
            })
          );
        },
        function(err, logs) {
          if (err) {
            return done(err);
          }
          done(null, logs);
        }
      );
    });
  });
};

module.exports = BlockLogsSerializer;
