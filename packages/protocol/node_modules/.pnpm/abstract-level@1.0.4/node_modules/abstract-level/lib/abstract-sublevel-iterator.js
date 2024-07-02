'use strict'

const { AbstractIterator, AbstractKeyIterator, AbstractValueIterator } = require('../abstract-iterator')

const kUnfix = Symbol('unfix')
const kIterator = Symbol('iterator')
const kHandleOne = Symbol('handleOne')
const kHandleMany = Symbol('handleMany')
const kCallback = Symbol('callback')

// TODO: unfix natively if db supports it
class AbstractSublevelIterator extends AbstractIterator {
  constructor (db, options, iterator, unfix) {
    super(db, options)

    this[kIterator] = iterator
    this[kUnfix] = unfix
    this[kHandleOne] = this[kHandleOne].bind(this)
    this[kHandleMany] = this[kHandleMany].bind(this)
    this[kCallback] = null
  }

  [kHandleOne] (err, key, value) {
    const callback = this[kCallback]
    if (err) return callback(err)
    if (key !== undefined) key = this[kUnfix](key)
    callback(err, key, value)
  }

  [kHandleMany] (err, entries) {
    const callback = this[kCallback]
    if (err) return callback(err)

    for (const entry of entries) {
      const key = entry[0]
      if (key !== undefined) entry[0] = this[kUnfix](key)
    }

    callback(err, entries)
  }
}

class AbstractSublevelKeyIterator extends AbstractKeyIterator {
  constructor (db, options, iterator, unfix) {
    super(db, options)

    this[kIterator] = iterator
    this[kUnfix] = unfix
    this[kHandleOne] = this[kHandleOne].bind(this)
    this[kHandleMany] = this[kHandleMany].bind(this)
    this[kCallback] = null
  }

  [kHandleOne] (err, key) {
    const callback = this[kCallback]
    if (err) return callback(err)
    if (key !== undefined) key = this[kUnfix](key)
    callback(err, key)
  }

  [kHandleMany] (err, keys) {
    const callback = this[kCallback]
    if (err) return callback(err)

    for (let i = 0; i < keys.length; i++) {
      const key = keys[i]
      if (key !== undefined) keys[i] = this[kUnfix](key)
    }

    callback(err, keys)
  }
}

class AbstractSublevelValueIterator extends AbstractValueIterator {
  constructor (db, options, iterator) {
    super(db, options)
    this[kIterator] = iterator
  }
}

for (const Iterator of [AbstractSublevelIterator, AbstractSublevelKeyIterator]) {
  Iterator.prototype._next = function (callback) {
    this[kCallback] = callback
    this[kIterator].next(this[kHandleOne])
  }

  Iterator.prototype._nextv = function (size, options, callback) {
    this[kCallback] = callback
    this[kIterator].nextv(size, options, this[kHandleMany])
  }

  Iterator.prototype._all = function (options, callback) {
    this[kCallback] = callback
    this[kIterator].all(options, this[kHandleMany])
  }
}

for (const Iterator of [AbstractSublevelValueIterator]) {
  Iterator.prototype._next = function (callback) {
    this[kIterator].next(callback)
  }

  Iterator.prototype._nextv = function (size, options, callback) {
    this[kIterator].nextv(size, options, callback)
  }

  Iterator.prototype._all = function (options, callback) {
    this[kIterator].all(options, callback)
  }
}

for (const Iterator of [AbstractSublevelIterator, AbstractSublevelKeyIterator, AbstractSublevelValueIterator]) {
  Iterator.prototype._seek = function (target, options) {
    this[kIterator].seek(target, options)
  }

  Iterator.prototype._close = function (callback) {
    this[kIterator].close(callback)
  }
}

exports.AbstractSublevelIterator = AbstractSublevelIterator
exports.AbstractSublevelKeyIterator = AbstractSublevelKeyIterator
exports.AbstractSublevelValueIterator = AbstractSublevelValueIterator
