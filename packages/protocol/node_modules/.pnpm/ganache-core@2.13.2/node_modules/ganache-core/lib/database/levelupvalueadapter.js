// Warning: Wrote this because I wanted it, then didn't need it.
// May come in handy later. You've been warned. This might be bad/dead code.
var Sublevel = require("level-sublevel");

function LevelUpValueAdapter(name, db, serializer) {
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
  this.value_key = "value";
}

LevelUpValueAdapter.prototype.get = function(callback) {
  var self = this;

  this.db.get(this.value_key, function(err, val) {
    if (err) {
      if (err.notFound) {
        return callback(null, null);
      } else {
        return callback(err);
      }
    }

    self.serializer.decode(val, callback);
  });
};

LevelUpValueAdapter.prototype.set = function(value, callback) {
  var self = this;
  this.serializer.encode(value, function(err, encoded) {
    if (err) {
      return callback(err);
    }
    self.db.put(self.value_key, encoded, callback);
  });
};

LevelUpValueAdapter.prototype.del = function(callback) {
  this.db.del(this.value_key, callback);
};

module.exports = LevelUpValueAdapter;
