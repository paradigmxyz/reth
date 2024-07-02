var txserializer = require("./txserializer");
var async = require("async");
var Block = require("ethereumjs-block");

module.exports = {
  encode: function(block, done) {
    var encoded = block.toJSON(true);

    async.map(
      block.transactions,
      function(tx, finished) {
        txserializer.encode(tx, finished);
      },
      function(err, transactions) {
        if (err) {
          return done(err);
        }
        encoded.transactions = transactions;
        done(null, encoded);
      }
    );
  },
  decode: function(json, done) {
    var transactions = json.transactions;
    json.transactions = [];

    var block = new Block(json);

    async.eachSeries(
      transactions,
      function(txJson, finished) {
        txserializer.decode(txJson, function(err, tx) {
          if (err) {
            return finished(err);
          }
          block.transactions.push(tx);
          finished();
        });
      },
      function(err) {
        if (err) {
          return done(err);
        }

        done(null, block);
      }
    );
  }
};
