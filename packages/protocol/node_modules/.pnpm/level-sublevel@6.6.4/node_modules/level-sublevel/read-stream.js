var IteratorStream = require('level-iterator-stream');
var inherits = require('util').inherits;
var EncodingError = require('level-errors').EncodingError;

function wrapIterator(it, options, makeData) {
  return {
      next: function (callback) {
      it.next(function(err, key, value) {
        if (err) {
          return callback(err);
        }
        if (key === undefined && value === undefined) {
          return callback(err, key, value);
        }
        var data;
        try {
          data = makeData(key, value);
        } catch (err) {
          return callback(new EncodingError(err));
        }
        if (options.keys !== false && options.values === false) {
          return callback(err, data, value);
        }
        if (options.keys === false && options.values !== false) {
          return callback(err, key, data);
        }
        return callback(err, data.key, data.value);
      });
    },
    end: function end(callback) {
      return it.end(callback);
    }
  }
}

function ReadStream (options, makeData) {
  if (!(this instanceof ReadStream)) {
    return new ReadStream(options, makeData);
  }

  IteratorStream.call(this, null, options);

  this._waiting = false;
  this._makeData = makeData;
}

inherits(ReadStream, IteratorStream)

ReadStream.prototype.setIterator = function (it) {
  this._iterator = wrapIterator(it, this._options, this._makeData);
  if (this.destroyed) {
    this._iterator.end(function () {});
    return;
  }
  if (this._waiting) {
    this._waiting = false
    this._read();
    return;
  }
};

ReadStream.prototype._read = function () {
  if (!this._iterator) {
    this._waiting = true;
    return;
  }
  IteratorStream.prototype._read.call(this);
}

module.exports = ReadStream


