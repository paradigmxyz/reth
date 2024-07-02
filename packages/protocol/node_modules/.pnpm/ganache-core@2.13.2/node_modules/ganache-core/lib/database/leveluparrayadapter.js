var Sublevel = require("level-sublevel");
const { LevelUpOutOfRangeError, BlockOutOfRangeError } = require("../utils/errorhelper");

// Level up adapter that looks like an array. Doesn't support inserts.

function LevelUpArrayAdapter(name, db, serializer) {
  this.db = Sublevel(db);
  this.db = this.db.sublevel(name);
  this.name = name;
  this.serializer = serializer || {
    encode: function(val, callback) {
      callback(null, val);
    },
    decode: function(val, callback) {
      callback(null, val);
    }
  };
}

LevelUpArrayAdapter.prototype.length = function(callback) {
  this.db.get("length", function(err, result) {
    if (err) {
      if (err.notFound) {
        return callback(null, 0);
      } else {
        return callback(err);
      }
    }

    callback(null, result);
  });
};

LevelUpArrayAdapter.prototype._get = function(key, callback) {
  var self = this;
  this.db.get(key, function(err, val) {
    if (err) {
      return callback(err);
    }
    self.serializer.decode(val, callback);
  });
};

LevelUpArrayAdapter.prototype._put = function(key, value, callback) {
  var self = this;
  this.serializer.encode(value, function(err, encoded) {
    if (err) {
      return callback(err);
    }
    self.db.put(key, encoded, callback);
  });
};

LevelUpArrayAdapter.prototype.get = function(index, callback) {
  var self = this;

  this.length(function(err, length) {
    if (err) {
      return callback(err);
    }
    if (index >= length) {
      // index out of range
      const RangeError =
        self.name === "blocks"
          ? new BlockOutOfRangeError(index, length)
          : new LevelUpOutOfRangeError(self.name, index, length);
      return callback(RangeError);
    }
    self._get(index, callback);
  });
};

LevelUpArrayAdapter.prototype.push = function(val, callback) {
  var self = this;
  this.length(function(err, length) {
    if (err) {
      return callback(err);
    }

    // TODO: Do this in atomic batch.
    self._put(length + "", val, function(err) {
      if (err) {
        return callback(err);
      }
      self.db.put("length", length + 1, callback);
    });
  });
};

LevelUpArrayAdapter.prototype.pop = function(callback) {
  var self = this;

  this.length(function(err, length) {
    if (err) {
      return callback(err);
    }

    var newLength = length - 1;

    // TODO: Do this in atomic batch.
    self._get(newLength + "", function(err, val) {
      if (err) {
        return callback(err);
      }
      self.db.del(newLength + "", function(err) {
        if (err) {
          return callback(err);
        }
        self.db.put("length", newLength, function(err) {
          if (err) {
            return callback(err);
          }

          callback(null, val);
        });
      });
    });
  });
};

LevelUpArrayAdapter.prototype.last = function(callback) {
  var self = this;
  this.length(function(err, length) {
    if (err) {
      return callback(err);
    }

    if (length === 0) {
      return callback(null, null);
    }

    self._get(length - 1 + "", callback);
  });
};

LevelUpArrayAdapter.prototype.first = function(callback) {
  this._get("0", callback);
};

module.exports = LevelUpArrayAdapter;
