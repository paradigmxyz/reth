
var AbstractLevelDOWN = require('abstract-leveldown').AbstractLevelDOWN
var inherits          = require('util').inherits
var EventEmitter      = require('events').EventEmitter
var Cache             = require('lru-cache')
var defaultLeveldown

function CacheDOWN (location, leveldown) {
  var self = this
  if (!(this instanceof CacheDOWN))
    return new CacheDOWN(location, leveldown)

  if (typeof location === 'object') {
    leveldown = location
    location = undefined
  }

  leveldown = leveldown || defaultLeveldown
  this._down = new leveldown(location)
  this._cache = new Cache()
  ;['_open', '_close', '_iterator'].forEach(function (method) {
    if (method in self._down) {
      self[method] = self._down[method].bind(self._down)
    }
  })

  if (typeof location === 'undefined') {
    AbstractLevelDOWN.call(this)
  } else {
    AbstractLevelDOWN.call(this, location)
  }
}

inherits(CacheDOWN, AbstractLevelDOWN)

CacheDOWN.prototype._put = function (key, value, options, callback) {
  if (typeof value === 'undefined' || value === null) value = ''

  this._cachePut(key, value)
  return this._down._put.apply(this._down, arguments)
}


CacheDOWN.prototype._get = function (key, options, callback) {
  var self = this
  if (this._cacheHas(key)) {
    var val = this._cacheGet(key)
    if (options.asBuffer !== false && !this._isBuffer(val)) {
      val = new Buffer(String(val))
    }

    return process.nextTick(function () {
      callback(null, val)
    })
  }

  return this._down._get(key, options, function (err, val) {
    if (err) return callback(err)

    self._cachePut(key, val)
    callback(err, val)
  })
}


CacheDOWN.prototype._del = function (key, options, callback) {
  this._cacheDel(key)
  return this._down._del.apply(this._down, arguments)
}


CacheDOWN.prototype._batch = function (array, options, callback) {
  var self = this
  array.forEach(function (item) {
    if (item.type === 'del') {
      self._cacheDel(item.key)
    } else if (item.type === 'put') {
      self._cachePut(item.key, item.value)
    }
  })

  return this._down._batch.apply(this._down, arguments)
}


CacheDOWN.prototype._isBuffer = function (obj) {
  return Buffer.isBuffer(obj)
}


CacheDOWN.prototype.clearCache = function () {
  this._cache.reset()
  return this
}

CacheDOWN.prototype._cachePut = function (key, value) {
  this._cache.set(key, value)
}

CacheDOWN.prototype._cacheGet = function (key) {
  return this._cache.get(key)
}

CacheDOWN.prototype._cacheHas = function (key) {
  return this._cache.has(key)
}

CacheDOWN.prototype._cacheDel = function (key, value) {
  this._cache.del(key)
}

CacheDOWN.prototype.maxSize = function (size) {
  this._cache.max = size
  return this
}

CacheDOWN.setLeveldown = function (leveldown) {
  defaultLeveldown = leveldown
}

module.exports                     = CacheDOWN
module.exports.factory             = function factory () {
  var args = Array.prototype.slice.call(arguments)
  return function makeCacheDOWN () {
    return CacheDOWN.apply(null, args)
  }
}
