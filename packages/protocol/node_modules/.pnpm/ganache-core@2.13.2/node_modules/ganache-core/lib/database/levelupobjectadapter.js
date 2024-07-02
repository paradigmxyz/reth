var Sublevel = require("level-sublevel");
var async = require("async");

function LevelUpObjectAdapter(name, db, valueserializer, keyserializer, options) {
  this.db = Sublevel(db, options);
  this.db = this.db.sublevel(name);
  this.name = name;
  this.valueserializer = valueserializer || {
    encode: function(val, callback) {
      callback(null, val);
    },
    decode: function(val, callback) {
      callback(null, val);
    }
  };
  this.keyserializer = keyserializer || {
    encode: function(val, callback) {
      callback(null, val);
    },
    decode: function(val, callback) {
      callback(null, val);
    }
  };
}

LevelUpObjectAdapter.prototype.get = function(key, options, callback) {
  var self = this;

  if (typeof options === "function") {
    callback = options;
    options = {};
  }

  this.keyserializer.encode(key, function(err, encodedKey) {
    if (err) {
      return callback(err);
    }

    self.db.get(encodedKey, function(err, val) {
      if (err) {
        return callback(err);
      }

      self.valueserializer.decode(val, function(err, decodedValue) {
        if (err) {
          return callback(err);
        }

        callback(null, decodedValue);
      });
    });
  });
};

LevelUpObjectAdapter.prototype.put = function(key, value, options, callback) {
  var self = this;

  if (typeof options === "function") {
    callback = options;
    options = {};
  }

  this.keyserializer.encode(key, function(err, encodedKey) {
    if (err) {
      return callback(err);
    }

    self.valueserializer.encode(value, function(err, encoded) {
      if (err) {
        return callback(err);
      }

      self.db.put(encodedKey, encoded, callback);
    });
  });
};

LevelUpObjectAdapter.prototype.set = LevelUpObjectAdapter.prototype.put;

LevelUpObjectAdapter.prototype.del = function(key, callback) {
  var self = this;

  this.keyserializer.encode(key, function(err, encodedKey) {
    if (err) {
      return callback(err);
    }

    self.db.del(encodedKey, callback);
  });
};

LevelUpObjectAdapter.prototype.batch = function(array, options, callback) {
  var self = this;

  async.each(
    array,
    function(item, finished) {
      if (item.type === "put") {
        self.put(item.key, item.value, options, finished);
      } else if (item.type === "del") {
        self.del(item.key, finished);
      } else {
        finished(new Error("Unknown batch type", item.type));
      }
    },
    function(err) {
      if (err) {
        return callback(err);
      }
      callback();
    }
  );
};

LevelUpObjectAdapter.prototype.isOpen = function() {
  return true;
};

module.exports = LevelUpObjectAdapter;
