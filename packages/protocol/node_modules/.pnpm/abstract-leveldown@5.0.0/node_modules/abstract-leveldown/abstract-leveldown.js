/* Copyright (c) 2017 Rod Vagg, MIT License */

var xtend = require('xtend')
var AbstractIterator = require('./abstract-iterator')
var AbstractChainedBatch = require('./abstract-chained-batch')
var hasOwnProperty = Object.prototype.hasOwnProperty
var rangeOptions = 'start end gt gte lt lte'.split(' ')

function AbstractLevelDOWN (location) {
  if (!arguments.length || location === undefined) {
    throw new Error('constructor requires at least a location argument')
  }

  if (typeof location !== 'string') {
    throw new Error('constructor requires a location string argument')
  }

  this.location = location
  this.status = 'new'
}

AbstractLevelDOWN.prototype.open = function (options, callback) {
  var self = this
  var oldStatus = this.status

  if (typeof options === 'function') { callback = options }

  if (typeof callback !== 'function') {
    throw new Error('open() requires a callback argument')
  }

  if (typeof options !== 'object') { options = {} }

  options.createIfMissing = options.createIfMissing !== false
  options.errorIfExists = !!options.errorIfExists

  this.status = 'opening'
  this._open(options, function (err) {
    if (err) {
      self.status = oldStatus
      return callback(err)
    }
    self.status = 'open'
    callback()
  })
}

AbstractLevelDOWN.prototype._open = function (options, callback) {
  process.nextTick(callback)
}

AbstractLevelDOWN.prototype.close = function (callback) {
  var self = this
  var oldStatus = this.status

  if (typeof callback !== 'function') {
    throw new Error('close() requires a callback argument')
  }

  this.status = 'closing'
  this._close(function (err) {
    if (err) {
      self.status = oldStatus
      return callback(err)
    }
    self.status = 'closed'
    callback()
  })
}

AbstractLevelDOWN.prototype._close = function (callback) {
  process.nextTick(callback)
}

AbstractLevelDOWN.prototype.get = function (key, options, callback) {
  if (typeof options === 'function') { callback = options }

  if (typeof callback !== 'function') {
    throw new Error('get() requires a callback argument')
  }

  var err = this._checkKey(key, 'key')
  if (err) return process.nextTick(callback, err)

  key = this._serializeKey(key)

  if (typeof options !== 'object') { options = {} }

  options.asBuffer = options.asBuffer !== false

  this._get(key, options, callback)
}

AbstractLevelDOWN.prototype._get = function (key, options, callback) {
  process.nextTick(function () { callback(new Error('NotFound')) })
}

AbstractLevelDOWN.prototype.put = function (key, value, options, callback) {
  if (typeof options === 'function') { callback = options }

  if (typeof callback !== 'function') {
    throw new Error('put() requires a callback argument')
  }

  var err = this._checkKey(key, 'key')
  if (err) return process.nextTick(callback, err)

  key = this._serializeKey(key)
  value = this._serializeValue(value)

  if (typeof options !== 'object') { options = {} }

  this._put(key, value, options, callback)
}

AbstractLevelDOWN.prototype._put = function (key, value, options, callback) {
  process.nextTick(callback)
}

AbstractLevelDOWN.prototype.del = function (key, options, callback) {
  if (typeof options === 'function') { callback = options }

  if (typeof callback !== 'function') {
    throw new Error('del() requires a callback argument')
  }

  var err = this._checkKey(key, 'key')
  if (err) return process.nextTick(callback, err)

  key = this._serializeKey(key)

  if (typeof options !== 'object') { options = {} }

  this._del(key, options, callback)
}

AbstractLevelDOWN.prototype._del = function (key, options, callback) {
  process.nextTick(callback)
}

AbstractLevelDOWN.prototype.batch = function (array, options, callback) {
  if (!arguments.length) { return this._chainedBatch() }

  if (typeof options === 'function') { callback = options }

  if (typeof array === 'function') { callback = array }

  if (typeof callback !== 'function') {
    throw new Error('batch(array) requires a callback argument')
  }

  if (!Array.isArray(array)) {
    return process.nextTick(callback, new Error('batch(array) requires an array argument'))
  }

  if (!options || typeof options !== 'object') { options = {} }

  var serialized = new Array(array.length)

  for (var i = 0; i < array.length; i++) {
    if (typeof array[i] !== 'object' || array[i] === null) {
      return process.nextTick(callback, new Error('batch(array) element must be an object and not `null`'))
    }

    var e = xtend(array[i])

    if (e.type !== 'put' && e.type !== 'del') {
      return process.nextTick(callback, new Error("`type` must be 'put' or 'del'"))
    }

    var err = this._checkKey(e.key, 'key')
    if (err) return process.nextTick(callback, err)

    e.key = this._serializeKey(e.key)

    if (e.type === 'put') { e.value = this._serializeValue(e.value) }

    serialized[i] = e
  }

  this._batch(serialized, options, callback)
}

AbstractLevelDOWN.prototype._batch = function (array, options, callback) {
  process.nextTick(callback)
}

AbstractLevelDOWN.prototype._setupIteratorOptions = function (options) {
  options = cleanRangeOptions(options)

  options.reverse = !!options.reverse
  options.keys = options.keys !== false
  options.values = options.values !== false
  options.limit = 'limit' in options ? options.limit : -1
  options.keyAsBuffer = options.keyAsBuffer !== false
  options.valueAsBuffer = options.valueAsBuffer !== false

  return options
}

function cleanRangeOptions (options) {
  var result = {}

  for (var k in options) {
    if (!hasOwnProperty.call(options, k)) continue
    if (isRangeOption(k) && isEmptyRangeOption(options[k])) continue

    result[k] = options[k]
  }

  return result
}

function isRangeOption (k) {
  return rangeOptions.indexOf(k) !== -1
}

function isEmptyRangeOption (v) {
  return v === '' || v == null || isEmptyBuffer(v)
}

function isEmptyBuffer (v) {
  return Buffer.isBuffer(v) && v.length === 0
}

AbstractLevelDOWN.prototype.iterator = function (options) {
  if (typeof options !== 'object') { options = {} }
  options = this._setupIteratorOptions(options)
  return this._iterator(options)
}

AbstractLevelDOWN.prototype._iterator = function (options) {
  return new AbstractIterator(this)
}

AbstractLevelDOWN.prototype._chainedBatch = function () {
  return new AbstractChainedBatch(this)
}

AbstractLevelDOWN.prototype._serializeKey = function (key) {
  return Buffer.isBuffer(key) ? key : String(key)
}

AbstractLevelDOWN.prototype._serializeValue = function (value) {
  if (value == null) return ''
  return Buffer.isBuffer(value) || process.browser ? value : String(value)
}

AbstractLevelDOWN.prototype._checkKey = function (obj, type) {
  if (obj === null || obj === undefined) {
    return new Error(type + ' cannot be `null` or `undefined`')
  }

  if (Buffer.isBuffer(obj) && obj.length === 0) {
    return new Error(type + ' cannot be an empty Buffer')
  }

  if (String(obj) === '') {
    return new Error(type + ' cannot be an empty String')
  }
}

module.exports = AbstractLevelDOWN
