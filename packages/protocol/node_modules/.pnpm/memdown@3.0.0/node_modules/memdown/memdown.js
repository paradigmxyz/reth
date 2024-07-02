var inherits = require('inherits')
var AbstractLevelDOWN = require('abstract-leveldown').AbstractLevelDOWN
var AbstractIterator = require('abstract-leveldown').AbstractIterator
var ltgt = require('ltgt')
var createRBT = require('functional-red-black-tree')
var Buffer = require('safe-buffer').Buffer

// In Node, use global.setImmediate. In the browser, use a consistent
// microtask library to give consistent microtask experience to all browsers
var setImmediate = require('./immediate')

function gt (value) {
  return ltgt.compare(value, this._upperBound) > 0
}

function gte (value) {
  return ltgt.compare(value, this._upperBound) >= 0
}

function lt (value) {
  return ltgt.compare(value, this._upperBound) < 0
}

function lte (value) {
  return ltgt.compare(value, this._upperBound) <= 0
}

function MemIterator (db, options) {
  AbstractIterator.call(this, db)
  this._limit = options.limit

  if (this._limit === -1) this._limit = Infinity

  var tree = db._store

  this.keyAsBuffer = options.keyAsBuffer !== false
  this.valueAsBuffer = options.valueAsBuffer !== false
  this._reverse = options.reverse
  this._options = options
  this._done = 0

  if (!this._reverse) {
    this._incr = 'next'
    this._lowerBound = ltgt.lowerBound(options)
    this._upperBound = ltgt.upperBound(options)

    if (typeof this._lowerBound === 'undefined') {
      this._tree = tree.begin
    } else if (ltgt.lowerBoundInclusive(options)) {
      this._tree = tree.ge(this._lowerBound)
    } else {
      this._tree = tree.gt(this._lowerBound)
    }

    if (this._upperBound) {
      if (ltgt.upperBoundInclusive(options)) {
        this._test = lte
      } else {
        this._test = lt
      }
    }
  } else {
    this._incr = 'prev'
    this._lowerBound = ltgt.upperBound(options)
    this._upperBound = ltgt.lowerBound(options)

    if (typeof this._lowerBound === 'undefined') {
      this._tree = tree.end
    } else if (ltgt.upperBoundInclusive(options)) {
      this._tree = tree.le(this._lowerBound)
    } else {
      this._tree = tree.lt(this._lowerBound)
    }

    if (this._upperBound) {
      if (ltgt.lowerBoundInclusive(options)) {
        this._test = gte
      } else {
        this._test = gt
      }
    }
  }
}

inherits(MemIterator, AbstractIterator)

MemIterator.prototype._next = function (callback) {
  var key
  var value

  if (this._done++ >= this._limit) return setImmediate(callback)
  if (!this._tree.valid) return setImmediate(callback)

  key = this._tree.key
  value = this._tree.value

  if (!this._test(key)) return setImmediate(callback)

  if (this.keyAsBuffer && !Buffer.isBuffer(key)) {
    key = Buffer.from(String(key))
  }

  if (this.valueAsBuffer && !Buffer.isBuffer(value)) {
    value = Buffer.from(String(value))
  }

  this._tree[this._incr]()

  setImmediate(function callNext () {
    callback(null, key, value)
  })
}

MemIterator.prototype._test = function () {
  return true
}

function MemDOWN () {
  if (!(this instanceof MemDOWN)) return new MemDOWN()

  AbstractLevelDOWN.call(this, '')

  this._store = createRBT(ltgt.compare)
}

inherits(MemDOWN, AbstractLevelDOWN)

MemDOWN.prototype._open = function (options, callback) {
  var self = this
  setImmediate(function callNext () {
    callback(null, self)
  })
}

MemDOWN.prototype._serializeKey = function (key) {
  return key
}

MemDOWN.prototype._serializeValue = function (value) {
  return value == null ? '' : value
}

MemDOWN.prototype._put = function (key, value, options, callback) {
  var iter = this._store.find(key)

  if (iter.valid) {
    this._store = iter.update(value)
  } else {
    this._store = this._store.insert(key, value)
  }

  setImmediate(callback)
}

MemDOWN.prototype._get = function (key, options, callback) {
  var value = this._store.get(key)

  if (typeof value === 'undefined') {
    // 'NotFound' error, consistent with LevelDOWN API
    return setImmediate(function callNext () {
      callback(new Error('NotFound'))
    })
  }

  if (options.asBuffer !== false && !Buffer.isBuffer(value)) {
    value = Buffer.from(String(value))
  }

  setImmediate(function callNext () {
    callback(null, value)
  })
}

MemDOWN.prototype._del = function (key, options, callback) {
  this._store = this._store.remove(key)
  setImmediate(callback)
}

MemDOWN.prototype._batch = function (array, options, callback) {
  var i = -1
  var key
  var value
  var iter
  var len = array.length
  var tree = this._store

  while (++i < len) {
    key = array[i].key
    iter = tree.find(key)

    if (array[i].type === 'put') {
      value = array[i].value
      tree = iter.valid ? iter.update(value) : tree.insert(key, value)
    } else {
      tree = iter.remove()
    }
  }

  this._store = tree

  setImmediate(callback)
}

MemDOWN.prototype._iterator = function (options) {
  return new MemIterator(this, options)
}

module.exports = MemDOWN.default = MemDOWN
