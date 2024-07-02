var LevelUpArrayAdapter = require("./database/leveluparrayadapter");
var LevelUpObjectAdapter = require("./database/levelupobjectadapter");
var levelup = require("levelup");
var encode = require("encoding-down");
var filedown = require("./database/filedown");
var cachedown = require("cachedown");
var txserializer = require("./database/txserializer");
var blockserializer = require("./database/blockserializer");
var bufferserializer = require("./database/bufferserializer");
var BlockLogsSerializer = require("./database/blocklogsserializer");
var ReceiptSerializer = require("./database/receiptserializer");
var tmp = require("tmp");

function Database(options) {
  this.options = options;
  this.directory = null;
}

Database.prototype.initialize = function(callback) {
  var self = this;

  function getDir(cb) {
    if (self.options.db_path) {
      cb(null, self.options.db_path);
    } else {
      tmp.dir(cb);
    }
  }

  getDir(function(err, directory) {
    if (err) {
      return callback(err);
    }
    const levelupOptions = { valueEncoding: "json" };
    if (self.options.db) {
      const store = self.options.db;
      levelup(store, levelupOptions, finishInitializing);
    } else {
      self.directory = directory;
      const store = encode(cachedown(directory, filedown).maxSize(100), levelupOptions);
      levelup(store, {}, finishInitializing);
    }
  });

  function finishInitializing(err, db) {
    if (err) {
      return callback(err);
    }

    self.db = db;

    // Blocks, keyed by array index (not necessarily by block number) (0-based)
    self.blocks = new LevelUpArrayAdapter("blocks", self.db, blockserializer);

    // Logs triggered in each block, keyed by block id (ids in the blocks array; not necessarily block number) (0-based)
    self.blockLogs = new LevelUpArrayAdapter("blockLogs", self.db, new BlockLogsSerializer(self));

    // Block hashes -> block ids (ids in the blocks array; not necessarily block number) for quick lookup
    self.blockHashes = new LevelUpObjectAdapter("blockHashes", self.db);

    // Transaction hash -> transaction objects
    self.transactions = new LevelUpObjectAdapter("transactions", self.db, txserializer);

    // Transaction hash -> transaction receipts
    self.transactionReceipts = new LevelUpObjectAdapter("transactionReceipts", self.db, new ReceiptSerializer(self));

    self.trie_db = new LevelUpObjectAdapter("trie_db", self.db, bufferserializer, bufferserializer);

    callback();
  }
};

Database.prototype.close = function(callback) {
  callback();
};

module.exports = Database;
