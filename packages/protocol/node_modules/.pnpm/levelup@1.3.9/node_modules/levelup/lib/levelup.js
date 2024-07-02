/* Copyright (c) 2012-2016 LevelUP contributors
 * See list at <https://github.com/level/levelup#contributing>
 * MIT License
 * <https://github.com/level/levelup/blob/master/LICENSE.md>
 */

var EventEmitter = require('events').EventEmitter
var inherits = require('util').inherits
var deprecate = require('util').deprecate
var extend = require('xtend')
var prr = require('prr')
var DeferredLevelDOWN = require('deferred-leveldown')
var IteratorStream = require('level-iterator-stream')
var Batch = require('./batch')
var Codec = require('level-codec')
var getLevelDOWN = require('./leveldown')
var errors = require('level-errors')
var util = require('./util')

var WriteError = errors.WriteError
var ReadError = errors.ReadError
var NotFoundError = errors.NotFoundError
var OpenError = errors.OpenError
var EncodingError = errors.EncodingError
var InitializationError = errors.InitializationError
var LevelUPError = errors.LevelUPError

var getOptions = util.getOptions
var defaultOptions = util.defaultOptions
var dispatchError = util.dispatchError

function getCallback (options, callback) {
  return typeof options === 'function' ? options : callback
}

// Possible LevelUP#_status values:
//  - 'new'     - newly created, not opened or closed
//  - 'opening' - waiting for the database to be opened, post open()
//  - 'open'    - successfully opened the database, available for use
//  - 'closing' - waiting for the database to be closed, post close()
//  - 'closed'  - database has been successfully closed, should not be
//                 used except for another open() operation

function LevelUP (location, options, callback) {
  if (!(this instanceof LevelUP)) { return new LevelUP(location, options, callback) }

  var error

  EventEmitter.call(this)
  this.setMaxListeners(Infinity)

  if (typeof location === 'function') {
    options = typeof options === 'object' ? options : {}
    options.db = location
    location = null
  } else if (typeof location === 'object' && typeof location.db === 'function') {
    options = location
    location = null
  }

  if (typeof options === 'function') {
    callback = options
    options = {}
  }

  if ((!options || typeof options.db !== 'function') && typeof location !== 'string') {
    error = new InitializationError(
        'Must provide a location for the database')
    if (callback) {
      return process.nextTick(function () {
        callback(error)
      })
    }
    throw error
  }

  options = getOptions(options)
  this.options = extend(defaultOptions, options)
  this._codec = new Codec(this.options)
  this._status = 'new'
  // set this.location as enumerable but not configurable or writable
  prr(this, 'location', location, 'e')

  this.open(callback)
}

inherits(LevelUP, EventEmitter)

LevelUP.prototype.open = function (callback) {
  var self = this
  var dbFactory
  var db

  if (this.isOpen()) {
    if (callback) { process.nextTick(function () { callback(null, self) }) }
    return this
  }

  if (this._isOpening()) {
    return callback && this.once('open', function () { callback(null, self) })
  }

  this.emit('opening')
  this._status = 'opening'
  this.db = new DeferredLevelDOWN(this.location)

  if (typeof this.options.db !== 'function' &&
      typeof getLevelDOWN !== 'function') {
    throw new LevelUPError('missing db factory, you need to set options.db')
  }

  dbFactory = this.options.db || getLevelDOWN()
  db = dbFactory(this.location)

  db.open(this.options, function (err) {
    if (err) {
      return dispatchError(self, new OpenError(err), callback)
    }
    self.db.setDb(db)
    self.db = db
    self._status = 'open'
    if (callback) { callback(null, self) }
    self.emit('open')
    self.emit('ready')
  })
}

LevelUP.prototype.close = function (callback) {
  var self = this

  if (this.isOpen()) {
    this._status = 'closing'
    this.db.close(function () {
      self._status = 'closed'
      self.emit('closed')
      if (callback) { callback.apply(null, arguments) }
    })
    this.emit('closing')
    this.db = new DeferredLevelDOWN(this.location)
  } else if (this._status === 'closed' && callback) {
    return process.nextTick(callback)
  } else if (this._status === 'closing' && callback) {
    this.once('closed', callback)
  } else if (this._isOpening()) {
    this.once('open', function () {
      self.close(callback)
    })
  }
}

LevelUP.prototype.isOpen = function () {
  return this._status === 'open'
}

LevelUP.prototype._isOpening = function () {
  return this._status === 'opening'
}

LevelUP.prototype.isClosed = function () {
  return (/^clos/).test(this._status)
}

function maybeError (db, options, callback) {
  if (!db._isOpening() && !db.isOpen()) {
    dispatchError(db, new ReadError('Database is not open'), callback)
    return true
  }
}

function writeError (db, message, callback) {
  dispatchError(db, new WriteError(message), callback)
}

function readError (db, message, callback) {
  dispatchError(db, new ReadError(message), callback)
}

LevelUP.prototype.get = function (key_, options, callback) {
  var self = this
  var key

  callback = getCallback(options, callback)

  if (maybeError(this, options, callback)) { return }

  if (key_ === null || key_ === undefined || typeof callback !== 'function') {
    return readError(this, 'get() requires key and callback arguments', callback)
  }

  options = util.getOptions(options)
  key = this._codec.encodeKey(key_, options)

  options.asBuffer = this._codec.valueAsBuffer(options)

  this.db.get(key, options, function (err, value) {
    if (err) {
      if ((/notfound/i).test(err) || err.notFound) {
        err = new NotFoundError(
            'Key not found in database [' + key_ + ']', err)
      } else {
        err = new ReadError(err)
      }
      return dispatchError(self, err, callback)
    }
    if (callback) {
      try {
        value = self._codec.decodeValue(value, options)
      } catch (e) {
        return callback(new EncodingError(e))
      }
      callback(null, value)
    }
  })
}

LevelUP.prototype.put = function (key_, value_, options, callback) {
  var self = this
  var key
  var value

  callback = getCallback(options, callback)

  if (key_ === null || key_ === undefined) { return writeError(this, 'put() requires a key argument', callback) }

  if (maybeError(this, options, callback)) { return }

  options = getOptions(options)
  key = this._codec.encodeKey(key_, options)
  value = this._codec.encodeValue(value_, options)

  this.db.put(key, value, options, function (err) {
    if (err) {
      return dispatchError(self, new WriteError(err), callback)
    }
    self.emit('put', key_, value_)
    if (callback) { callback() }
  })
}

LevelUP.prototype.del = function (key_, options, callback) {
  var self = this
  var key

  callback = getCallback(options, callback)

  if (key_ === null || key_ === undefined) { return writeError(this, 'del() requires a key argument', callback) }

  if (maybeError(this, options, callback)) { return }

  options = getOptions(options)
  key = this._codec.encodeKey(key_, options)

  this.db.del(key, options, function (err) {
    if (err) {
      return dispatchError(self, new WriteError(err), callback)
    }
    self.emit('del', key_)
    if (callback) { callback() }
  })
}

LevelUP.prototype.batch = function (arr_, options, callback) {
  var self = this
  var arr

  if (!arguments.length) { return new Batch(this, this._codec) }

  callback = getCallback(options, callback)

  if (!Array.isArray(arr_)) { return writeError(this, 'batch() requires an array argument', callback) }

  if (maybeError(this, options, callback)) { return }

  options = getOptions(options)
  arr = self._codec.encodeBatch(arr_, options)
  arr = arr.map(function (op) {
    if (!op.type && op.key !== undefined && op.value !== undefined) { op.type = 'put' }
    return op
  })

  this.db.batch(arr, options, function (err) {
    if (err) {
      return dispatchError(self, new WriteError(err), callback)
    }
    self.emit('batch', arr_)
    if (callback) { callback() }
  })
}

LevelUP.prototype.approximateSize = deprecate(function (start_, end_, options, callback) {
  var self = this
  var start
  var end

  callback = getCallback(options, callback)

  options = getOptions(options)

  if (start_ === null || start_ === undefined || end_ === null ||
      end_ === undefined || typeof callback !== 'function') {
    return readError(this, 'approximateSize() requires start, end and callback arguments', callback)
  }

  start = this._codec.encodeKey(start_, options)
  end = this._codec.encodeKey(end_, options)

  this.db.approximateSize(start, end, function (err, size) {
    if (err) {
      return dispatchError(self, new OpenError(err), callback)
    } else if (callback) {
      callback(null, size)
    }
  })
}, 'db.approximateSize() is deprecated. Use db.db.approximateSize() instead')

LevelUP.prototype.readStream =
LevelUP.prototype.createReadStream = function (options) {
  options = extend({keys: true, values: true}, this.options, options)

  options.keyEncoding = options.keyEncoding
  options.valueEncoding = options.valueEncoding

  options = this._codec.encodeLtgt(options)
  options.keyAsBuffer = this._codec.keyAsBuffer(options)
  options.valueAsBuffer = this._codec.valueAsBuffer(options)

  if (typeof options.limit !== 'number') { options.limit = -1 }

  return new IteratorStream(this.db.iterator(options), extend(options, {
    decoder: this._codec.createStreamDecoder(options)
  }))
}

LevelUP.prototype.keyStream =
LevelUP.prototype.createKeyStream = function (options) {
  return this.createReadStream(extend(options, { keys: true, values: false }))
}

LevelUP.prototype.valueStream =
LevelUP.prototype.createValueStream = function (options) {
  return this.createReadStream(extend(options, { keys: false, values: true }))
}

LevelUP.prototype.toString = function () {
  return 'LevelUP'
}

function utilStatic (name) {
  return function (location, callback) {
    getLevelDOWN()[name](location, callback || function () {})
  }
}

module.exports = LevelUP
module.exports.errors = require('level-errors')
module.exports.destroy = deprecate(
  utilStatic('destroy'),
  'levelup.destroy() is deprecated. Use leveldown.destroy() instead'
)
module.exports.repair = deprecate(
  utilStatic('repair'),
  'levelup.repair() is deprecated. Use leveldown.repair() instead'
)
